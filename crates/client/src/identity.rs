//! Identity and addressing types: actors, orgs, data contexts, API origins,
//! store slot keys and screen positions.
//!
//! Formats follow IAM 4.0.0 (BLUEPRINT §2.1): Silicons are `si:<handle>`
//! (handle 3–50), Carbons are `c:<handle>` (handle 3–30), organizations are
//! bare handles (3–50); every handle is `[a-z0-9_-]`.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use url::Url;
use uuid::Uuid;

use crate::{
    Secret,
    error::{Error, ErrorCode, Result},
};

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

/// A bare organization handle, e.g. `tos` (3–50 of `a-z 0-9 _ -`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OrgId(String);

impl OrgId {
    /// Parses and validates an organization handle.
    ///
    /// # Errors
    /// `invalid_input` for an empty, too long or badly formed handle.
    pub fn parse(s: &str) -> Result<Self> {
        if handle_ok(s, 3, 50) {
            Ok(Self(s.to_owned()))
        } else {
            Err(Error::invalid_input(format!(
                "`{}` is not an organization handle; expected 3–50 of a-z 0-9 _ - (for example `tos`)",
                truncate_for_message(s)
            )))
        }
    }

    /// The handle.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
string_newtype_serde!(OrgId);

/// Chooses the org for org-specific calls: `--org`, then `SILICON_ORG`, then
/// the session slot's org (BLUEPRINT §7.1). An explicitly empty value is an
/// error, never a fallback.
///
/// # Errors
/// `invalid_input` when a supplied value is empty or malformed.
pub fn resolve_org(
    flag: Option<&str>,
    env: Option<&str>,
    slot: Option<&OrgId>,
) -> Result<Option<OrgId>> {
    for (value, name) in [(flag, "--org"), (env, "SILICON_ORG")] {
        if let Some(v) = value {
            if v.is_empty() {
                return Err(Error::invalid_input(format!(
                    "{name} is set but empty; pass an organization handle such as `tos`, or unset it"
                )));
            }
            return OrgId::parse(v).map(Some);
        }
    }
    Ok(slot.cloned())
}

/// The data context a store slot, a request or a bubble belongs to.
///
/// Production and every testing environment are strictly separate: separate
/// session slots, separate peekd rows, separate backend databases.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Context {
    /// The production plane.
    Production,
    /// A Honeycomb testing environment, by its UUID.
    Testing(Uuid),
}

impl Context {
    /// Parses `"production"` or a testing environment UUID.
    ///
    /// # Errors
    /// `invalid_input` for anything else (including the nil UUID).
    pub fn parse(s: &str) -> Result<Self> {
        if s == "production" {
            return Ok(Self::Production);
        }
        match Uuid::parse_str(s) {
            Ok(id) if !id.is_nil() => Ok(Self::Testing(id)),
            _ => Err(Error::invalid_input(format!(
                "`{}` is not a context; expected `production` or a testing environment UUID",
                truncate_for_message(s)
            ))),
        }
    }

    /// `production` or the hyphenated UUID.
    #[must_use]
    pub fn as_string(&self) -> String {
        match self {
            Self::Production => "production".to_owned(),
            Self::Testing(id) => id.hyphenated().to_string(),
        }
    }

    /// The testing environment UUID, if any.
    #[must_use]
    pub fn testing_id(&self) -> Option<Uuid> {
        match self {
            Self::Production => None,
            Self::Testing(id) => Some(*id),
        }
    }

    /// Whether this is a testing context.
    #[must_use]
    pub fn is_testing(&self) -> bool {
        matches!(self, Self::Testing(_))
    }

    /// The coarse context recorded in Ting payloads.
    #[must_use]
    pub fn data_context(&self) -> DataContext {
        match self {
            Self::Production => DataContext::Production,
            Self::Testing(_) => DataContext::Testing,
        }
    }
}

impl fmt::Display for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Production => f.write_str("production"),
            Self::Testing(id) => write!(f, "{}", id.hyphenated()),
        }
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
    /// Any testing environment.
    Testing,
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

/// A peek testing app secret (`ask_` followed by 43 of `A-Z a-z 0-9 _ -`).
///
/// It is sent to peek-server as `X-Testing-Environment-Key`. It is never read
/// from `IAM_TEST_APP_SECRET` or `IAM_TEST_KEY`, which belong to Ting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestingSecret(Secret);

impl TestingSecret {
    /// Validates a testing secret.
    ///
    /// # Errors
    /// `testing_secret_invalid` with a hint that never echoes the value.
    pub fn parse(s: &str) -> Result<Self> {
        let ok = s.len() == 47
            && s.starts_with("ask_")
            && s[4..]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if ok {
            Ok(Self(Secret::new(s)))
        } else {
            Err(Error::new(
                ErrorCode::TestingSecretInvalid,
                format!(
                    "the testing app secret is malformed ({} characters); expected the peek test app secret `ask_…` (47 characters) from `honeycomb --test <env> apps rotate-secret 'peek'`",
                    s.chars().count()
                ),
            )
            .with_hint("pass it on stdin: printf %s \"$SECRET\" | peek --app-secret-file - <command>"))
        }
    }

    /// The secret.
    #[must_use]
    pub fn secret(&self) -> &Secret {
        &self.0
    }
}

impl Serialize for TestingSecret {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> Deserialize<'de> for TestingSecret {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = Secret::deserialize(d)?;
        Self::parse(s.expose()).map_err(|e| serde::de::Error::custom(e.message().to_owned()))
    }
}

/// Parses the value of `--test` / `SILICON_PEEK_TEST`: a testing environment
/// UUID. A raw secret is refused so it never lands in shell history or `ps`.
///
/// # Errors
/// `testing_secret_invalid` for a secret, `invalid_input` for anything else.
pub fn parse_test_selector(s: &str) -> Result<Uuid> {
    if s.starts_with("ask_") {
        return Err(Error::new(
            ErrorCode::TestingSecretInvalid,
            "--test takes a testing environment UUID, not a secret; the value you passed looks like an app secret",
        )
        .with_hint("use --app-secret-file - and pass the secret on stdin"));
    }
    match Uuid::parse_str(s) {
        Ok(id) if !id.is_nil() => Ok(id),
        _ => Err(Error::invalid_input(format!(
            "--test `{}` is not a testing environment UUID",
            truncate_for_message(s)
        ))
        .with_hint(
            "list saved environments with `peek status`, or discover one with --app-secret-file -",
        )),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_ids() {
        assert!(ActorId::parse("si:cleanup").is_ok());
        assert!(ActorId::parse("c:shubham").is_ok());
        assert!(ActorId::parse("si:ab").is_err());
        assert!(ActorId::parse(&format!("si:{}", "a".repeat(50))).is_ok());
        assert!(ActorId::parse(&format!("si:{}", "a".repeat(51))).is_err());
        assert!(ActorId::parse(&format!("c:{}", "a".repeat(30))).is_ok());
        assert!(ActorId::parse(&format!("c:{}", "a".repeat(31))).is_err());
        assert!(ActorId::parse("si:Upper").is_err());
        assert!(ActorId::parse("tos>peek").is_err());
        assert!(ActorId::parse("x:abc").is_err());
        let a = ActorId::parse("si:dj-bot").ok();
        assert_eq!(
            a.as_ref().map(ActorId::actor_type),
            Some(ActorType::Silicon)
        );
        assert_eq!(a.as_ref().map(ActorId::handle), Some("dj-bot"));
    }

    #[test]
    fn org_resolution_precedence() -> Result<()> {
        let slot = OrgId::parse("tos")?;
        assert_eq!(
            resolve_org(Some("acme"), Some("zzz"), Some(&slot))?.map(|o| o.0),
            Some("acme".into())
        );
        assert_eq!(
            resolve_org(None, Some("zzz"), Some(&slot))?.map(|o| o.0),
            Some("zzz".into())
        );
        assert_eq!(
            resolve_org(None, None, Some(&slot))?.map(|o| o.0),
            Some("tos".into())
        );
        assert!(resolve_org(Some(""), None, Some(&slot)).is_err());
        assert!(resolve_org(None, Some(""), Some(&slot)).is_err());
        assert!(resolve_org(None, None, None)?.is_none());
        Ok(())
    }

    #[test]
    fn contexts_and_slot_keys() -> Result<()> {
        let id = Uuid::now_v7();
        assert_eq!(Context::parse("production")?, Context::Production);
        assert_eq!(Context::parse(&id.to_string())?, Context::Testing(id));
        assert!(Context::parse("00000000-0000-0000-0000-000000000000").is_err());
        assert!(Context::parse("testing").is_err());
        let key = SlotKey::new(ApiUrl::production(), Context::Production);
        assert_eq!(
            key.as_string(),
            "https://backend.peek.teamofsilicons.com#production"
        );
        assert_eq!(SlotKey::parse(&key.as_string())?, key);
        let test = SlotKey::new(
            ApiUrl::parse("http://127.0.0.1:8080/")?,
            Context::Testing(id),
        );
        assert_eq!(test.as_string(), format!("http://127.0.0.1:8080#{id}"));
        Ok(())
    }

    #[test]
    fn api_urls() {
        assert_eq!(
            ApiUrl::parse("https://backend.peek.teamofsilicons.com/")
                .map(|u| u.0)
                .ok(),
            Some("https://backend.peek.teamofsilicons.com".into())
        );
        assert!(ApiUrl::parse("http://localhost:3000").is_ok());
        assert!(ApiUrl::parse("http://[::1]:3000").is_ok());
        assert!(ApiUrl::parse("http://example.com").is_err());
        assert!(ApiUrl::parse("https://example.com/api").is_err());
        assert!(ApiUrl::parse("https://u:p@example.com").is_err());
        assert!(ApiUrl::parse("https://example.com?x=1").is_err());
        assert!(ApiUrl::parse("ftp://example.com").is_err());
        assert!(ApiUrl::parse("not a url").is_err());
    }

    #[test]
    fn testing_secrets_and_selectors() {
        let good = format!("ask_{}", "A".repeat(43));
        assert!(TestingSecret::parse(&good).is_ok());
        assert!(TestingSecret::parse(&good[..46]).is_err());
        let e = TestingSecret::parse("ask_bad!").err();
        assert!(e.is_some_and(|e| !e.message().contains("ask_bad!")));
        assert!(parse_test_selector(&good).is_err());
        assert!(parse_test_selector(&Uuid::now_v7().to_string()).is_ok());
        assert!(parse_test_selector("nope").is_err());
    }

    #[test]
    fn slots_and_sides() -> Result<()> {
        assert!(SlotIndex::new(0).is_err());
        assert!(SlotIndex::new(9).is_err());
        assert_eq!(SlotIndex::new(1)?.side(), Side::Top);
        assert_eq!(SlotIndex::new(3)?.side(), Side::Right);
        assert_eq!(SlotIndex::new(5)?.side(), Side::Bottom);
        assert_eq!(SlotIndex::new(8)?.side(), Side::TopLeft);
        let info = SlotInfo::from(SlotIndex::new(2)?);
        assert_eq!(
            serde_json::to_string(&info).map_err(|e| Error::internal(e.to_string()))?,
            r#"{"index":2,"side":"top-right"}"#
        );
        assert!(serde_json::from_str::<SlotIndex>("9").is_err());
        Ok(())
    }
}
