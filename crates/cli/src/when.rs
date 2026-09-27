//! `--at` / `--expires-at` date-times and `--tz` (0.1.2 contract §5.3).
//!
//! Grammar (the case of `T` and `Z` does not matter):
//!
//! ```text
//! VALUE  = DATE SEP TIME [OFFSET] | TIME [OFFSET]
//! DATE   = YYYY-MM-DD            SEP = "T" | " "
//! TIME   = HH:MM[:SS[.f{1,9}]]   (24 h)
//! OFFSET = "Z" | ±HH:MM | ±HHMM | ±HH
//! ```
//!
//! A time without a date means today, in the zone. The zone is the value's
//! offset when it has one (then `--tz` is ignored for that value), else
//! `--tz`, else the Mac's system zone: what the menu-bar clock shows, read
//! from `/etc/localtime` and never from `$TZ`. A local time that does not
//! exist (the hour clocks skip) is refused; an ambiguous one (the hour that
//! repeats) is the earlier.

use std::{fmt::Write as _, path::Path};

use jiff::{
    civil::{Date, DateTime, Time},
    tz::{AmbiguousOffset, Offset, TimeZone},
};
use serde_json::json;
use silicon_peek_client::{Error, Result, schema::send::check_tz_name, timestamp::Timestamp};

/// Where the Mac keeps its time zone.
pub const LOCALTIME: &str = "/etc/localtime";

/// A resolved date-time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    /// The instant.
    pub at: Timestamp,
    /// The IANA zone it was read in; `None` when the value carried an offset
    /// (or the Mac's zone has no known name).
    pub tz: Option<String>,
}

/// A `--tz` value, parsed once.
#[derive(Clone, Debug)]
pub struct TzArg {
    /// The IANA name as given.
    pub name: String,
    /// The zone.
    pub zone: TimeZone,
}

/// The Mac's system zone.
#[derive(Clone, Debug)]
pub struct LocalZone {
    /// The zone.
    pub zone: TimeZone,
    /// Its IANA name, when known.
    pub name: Option<String>,
    /// `/etc/localtime` could not be read: UTC is used (the send carries the
    /// `timezone_fallback_utc` warning).
    pub fallback_utc: bool,
}

const EXAMPLES: &str = "use 2026-09-27T18:00, \"2026-09-27 18:00\", 18:00, or add Z / +05:30";

/// Parses `--tz`: an IANA zone from the time zone database.
///
/// # Errors
/// `invalid_input` with field `--tz`.
pub fn parse_tz(raw: &str) -> Result<TzArg> {
    let name = raw.trim();
    let bad = || {
        Error::invalid_input(format!("`--tz {name}` is not an IANA time zone"))
            .with_hint("e.g. Asia/Kolkata, Europe/Berlin, America/New_York, UTC")
            .with_details(json!({"field": "--tz", "value": raw}))
    };
    check_tz_name(name).map_err(|_| bad())?;
    let zone = jiff::tz::db().get(name).map_err(|_| bad())?;
    Ok(TzArg {
        name: name.to_owned(),
        zone,
    })
}

/// The Mac's system zone (see [`local_zone_from`] on [`LOCALTIME`]).
#[must_use]
pub fn mac_local_zone() -> LocalZone {
    local_zone_from(Path::new(LOCALTIME))
}

/// The zone `localtime` names: its symlink target after `zoneinfo/`, looked
/// up in the time zone database; else the file's own `TZif` data (no name);
/// else UTC with `fallback_utc`.
#[must_use]
pub fn local_zone_from(localtime: &Path) -> LocalZone {
    if let Ok(target) = std::fs::read_link(localtime) {
        let target = target.to_string_lossy().into_owned();
        if let Some((_, name)) = target.rsplit_once("zoneinfo/")
            && let Ok(zone) = jiff::tz::db().get(name)
        {
            return LocalZone {
                zone,
                name: Some(name.to_owned()),
                fallback_utc: false,
            };
        }
    }
    if let Ok(bytes) = std::fs::read(localtime)
        && let Ok(zone) = TimeZone::tzif("Local", &bytes)
    {
        return LocalZone {
            zone,
            name: None,
            fallback_utc: false,
        };
    }
    LocalZone {
        zone: TimeZone::UTC,
        name: None,
        fallback_utc: true,
    }
}

/// A parsed value before a zone is applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parsed {
    /// The date, when given.
    pub date: Option<Date>,
    /// The time.
    pub time: Time,
    /// The offset, when given.
    pub offset: Option<Offset>,
}

fn digits(s: &str, n: usize) -> Option<i32> {
    (s.len() == n && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok())
        .flatten()
}

fn narrow<T: TryFrom<i32>>(v: i32) -> Option<T> {
    T::try_from(v).ok()
}

/// The offset part of a value: absent, or an offset.
#[derive(Clone, Copy, Debug)]
enum OffsetPart {
    None,
    Some(Offset),
}

impl OffsetPart {
    fn into_option(self) -> Option<Offset> {
        match self {
            Self::None => None,
            Self::Some(o) => Some(o),
        }
    }
}

fn parse_offset(s: &str) -> Option<OffsetPart> {
    if s.is_empty() {
        return Some(OffsetPart::None);
    }
    if s.eq_ignore_ascii_case("z") {
        return Some(OffsetPart::Some(Offset::UTC));
    }
    let (sign, rest) = match s.as_bytes()[0] {
        b'+' => (1, &s[1..]),
        b'-' => (-1, &s[1..]),
        _ => return None,
    };
    let (h, m) = match rest.len() {
        2 => (digits(rest, 2)?, 0),
        4 => (digits(&rest[..2], 2)?, digits(&rest[2..], 2)?),
        5 if rest.as_bytes()[2] == b':' => (digits(&rest[..2], 2)?, digits(&rest[3..], 2)?),
        _ => return None,
    };
    if h > 23 || m > 59 {
        return None;
    }
    Offset::from_seconds(sign * (h * 3600 + m * 60))
        .ok()
        .map(OffsetPart::Some)
}

/// `HH:MM[:SS[.f{1,9}]]` then the offset.
fn parse_time_and_offset(s: &str) -> Option<(Time, Option<Offset>)> {
    let b = s.as_bytes();
    if b.len() < 5 || b[2] != b':' {
        return None;
    }
    let hour = digits(&s[..2], 2)?;
    let minute = digits(&s[3..5], 2)?;
    let mut i = 5;
    let mut second = 0;
    let mut nanos = 0;
    if b.get(i) == Some(&b':') {
        second = digits(s.get(i + 1..i + 3)?, 2)?;
        i += 3;
        if b.get(i) == Some(&b'.') {
            let start = i + 1;
            let mut end = start;
            while end < b.len() && b[end].is_ascii_digit() {
                end += 1;
            }
            let frac = &s[start..end];
            if frac.is_empty() || frac.len() > 9 {
                return None;
            }
            nanos = format!("{frac:0<9}").parse().ok()?;
            i = end;
        }
    }
    let offset = parse_offset(&s[i..])?.into_option();
    let time = Time::new(narrow(hour)?, narrow(minute)?, narrow(second)?, nanos).ok()?;
    Some((time, offset))
}

/// Parses the grammar (no zone applied yet).
///
/// # Errors
/// `invalid_input` naming `flag`, with `details.field` and `details.value`.
pub fn parse(flag: &str, raw: &str) -> Result<Parsed> {
    let s = raw.trim();
    let unparsable = || {
        Error::invalid_input(format!("`{flag} {s}` is not a date-time; {EXAMPLES}"))
            .with_hint(format!(
                "{flag} 2026-09-27T18:00, {flag} \"2026-09-27 18:00\", {flag} 18:00, or with an offset: 18:00Z, 18:00+05:30"
            ))
            .with_details(json!({"field": flag, "value": raw}))
    };
    let b = s.as_bytes();
    let dated =
        b.len() >= 10 && b[..4].iter().all(u8::is_ascii_digit) && b[4] == b'-' && b[7] == b'-';
    if !dated {
        let (time, offset) = parse_time_and_offset(s).ok_or_else(unparsable)?;
        return Ok(Parsed {
            date: None,
            time,
            offset,
        });
    }
    let year = digits(&s[..4], 4).ok_or_else(unparsable)?;
    let month = digits(&s[5..7], 2).ok_or_else(unparsable)?;
    let day = digits(&s[8..10], 2).ok_or_else(unparsable)?;
    let date = Date::new(
        narrow(year).ok_or_else(unparsable)?,
        narrow(month).ok_or_else(unparsable)?,
        narrow(day).ok_or_else(unparsable)?,
    )
    .map_err(|_| unparsable())?;
    if b.len() == 10 {
        return Err(Error::invalid_input(format!(
            "`{flag} {s}` has no time; add one, e.g. {s}T09:00"
        ))
        .with_hint(format!("{flag} {s}T09:00"))
        .with_details(json!({"field": flag, "value": raw})));
    }
    if !matches!(b[10], b'T' | b't' | b' ') {
        return Err(unparsable());
    }
    let (time, offset) = parse_time_and_offset(&s[11..]).ok_or_else(unparsable)?;
    Ok(Parsed {
        date: Some(date),
        time,
        offset,
    })
}

/// Whether a value needs the Mac's zone (it has no offset and no `--tz`).
#[must_use]
pub fn needs_local(raw: &str, tz_flag: Option<&TzArg>) -> bool {
    tz_flag.is_none() && parse("--at", raw).is_ok_and(|p| p.offset.is_none())
}

fn to_jiff(ts: Timestamp) -> jiff::Timestamp {
    jiff::Timestamp::from_millisecond(ts.unix_ms()).unwrap_or(jiff::Timestamp::UNIX_EPOCH)
}

fn from_jiff(ts: jiff::Timestamp) -> Timestamp {
    Timestamp::from_unix_ms(ts.as_millisecond())
}

/// Resolves `raw` (for `flag`) to an instant in the future (see the module
/// grammar and zone rules).
///
/// # Errors
/// `invalid_input` (field `flag`): unparsable, date only, a nonexistent local
/// time (`details.reason = "nonexistent_local_time"`), or not after `now`.
pub fn resolve(
    flag: &str,
    raw: &str,
    tz_flag: Option<&TzArg>,
    local: &LocalZone,
    now: Timestamp,
) -> Result<Resolved> {
    let parsed = parse(flag, raw)?;
    let s = raw.trim();
    let (zone, name) = match (parsed.offset, tz_flag) {
        (Some(offset), _) => (TimeZone::fixed(offset), None),
        (None, Some(t)) => (t.zone.clone(), Some(t.name.clone())),
        (None, None) => (local.zone.clone(), local.name.clone()),
    };
    let today = zone.to_datetime(to_jiff(now)).date();
    let date = parsed.date.unwrap_or(today);
    let dt = DateTime::from_parts(date, parsed.time);
    let ambiguous = zone.to_ambiguous_zoned(dt);
    if let AmbiguousOffset::Gap { .. } = ambiguous.offset() {
        let zone_name = name
            .clone()
            .unwrap_or_else(|| "the Mac's time zone".to_owned());
        return Err(Error::invalid_input(format!(
            "`{flag} {s}` does not exist in {zone_name} (clocks skip that hour)"
        ))
        .with_hint("pick a time outside the daylight-saving change")
        .with_details(json!({"field": flag, "value": raw, "reason": "nonexistent_local_time"})));
    }
    let at = ambiguous
        .earlier()
        .map_err(|e| {
            Error::invalid_input(format!("`{flag} {s}` cannot be placed in time: {e}"))
                .with_hint(format!("{flag} 2026-09-27T18:00Z"))
                .with_details(json!({"field": flag, "value": raw}))
        })?
        .timestamp();
    let at = from_jiff(at);
    if at <= now {
        let hint = match parsed.date {
            None => {
                let tomorrow = today
                    .tomorrow()
                    .map_or_else(|_| "tomorrow".to_owned(), |d| d.to_string());
                let hm = format!("{:02}:{:02}", parsed.time.hour(), parsed.time.minute());
                format!("for tomorrow use {flag} {tomorrow}T{hm}")
            }
            Some(_) => format!("give a time after now ({})", render_short(now, &zone)),
        };
        return Err(Error::invalid_input(format!(
            "`{flag} {s}` is in the past: {} (now {})",
            render_short(at, &zone),
            render_short(now, &zone)
        ))
        .with_hint(hint)
        .with_details(json!({"field": flag, "value": raw, "resolved": at, "now": now})));
    }
    Ok(Resolved { at, tz: name })
}

/// `2026-09-27 18:00 IST` in `zone` (seconds only when not zero).
#[must_use]
pub fn render_short(at: Timestamp, zone: &TimeZone) -> String {
    let ts = to_jiff(at);
    let dt = zone.to_datetime(ts);
    let info = zone.to_offset_info(ts);
    let seconds = if dt.second() == 0 {
        String::new()
    } else {
        format!(":{:02}", dt.second())
    };
    format!(
        "{} {:02}:{:02}{seconds} {}",
        dt.date(),
        dt.hour(),
        dt.minute(),
        info.abbreviation()
    )
}

/// `2026-09-27 18:00 IST`, plus ` (Asia/Kolkata)` when the zone's name is
/// known.
#[must_use]
pub fn render_local(at: Timestamp, zone: &TimeZone, name: Option<&str>) -> String {
    let short = render_short(at, zone);
    match name {
        Some(n) => format!("{short} ({n})"),
        None => short,
    }
}

/// `HH:MM` of `at` in `zone` when it falls on the same local day as `base`,
/// else the full short form (for `expires 18:30`).
#[must_use]
pub fn render_near(at: Timestamp, base: Timestamp, zone: &TimeZone) -> String {
    let dt = zone.to_datetime(to_jiff(at));
    if dt.date() == zone.to_datetime(to_jiff(base)).date() {
        format!("{:02}:{:02}", dt.hour(), dt.minute())
    } else {
        render_short(at, zone)
    }
}

/// A span as its two largest units (`2h 13m`, `9m 48s`, `17h`, `45s`,
/// `1d 2h`), dropping a zero second unit.
#[must_use]
pub fn span(ms: u64) -> String {
    let secs = ms / 1000;
    let units = [
        (secs / 86_400, "d"),
        (secs % 86_400 / 3600, "h"),
        (secs % 3600 / 60, "m"),
        (secs % 60, "s"),
    ];
    let first = units.iter().position(|(n, _)| *n > 0).unwrap_or(3);
    let mut out = format!("{}{}", units[first].0, units[first].1);
    if let Some((n, u)) = units.get(first + 1)
        && *n > 0
    {
        let _ = write!(out, " {n}{u}");
    }
    out
}

/// `in 2h 13m` / `in 45s` (or `5m ago`) from `now`.
#[must_use]
pub fn render_relative(at: Timestamp, now: Timestamp) -> String {
    let diff = at.unix_ms().saturating_sub(now.unix_ms());
    let ms = diff.unsigned_abs();
    if diff >= 0 {
        format!("in {}", span(ms))
    } else {
        format!("{} ago", span(ms))
    }
}

/// An age as its largest unit, floored: `12s`, `4m`, `2h`, `3d`.
#[must_use]
pub fn age(ms: u64) -> String {
    let secs = ms / 1000;
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kolkata() -> LocalZone {
        LocalZone {
            zone: jiff::tz::db().get("Asia/Kolkata").unwrap_or(TimeZone::UTC),
            name: Some("Asia/Kolkata".to_owned()),
            fallback_utc: false,
        }
    }

    fn now() -> Timestamp {
        Timestamp::parse("2026-09-27T06:12:00Z").unwrap_or(Timestamp::from_unix(0))
    }

    fn at(raw: &str, tz: Option<&TzArg>) -> Result<Resolved> {
        resolve("--at", raw, tz, &kolkata(), now())
    }

    fn utc(s: &str) -> Timestamp {
        Timestamp::parse(s).unwrap_or(Timestamp::from_unix(0))
    }

    fn field_and_value(e: &Error) -> (String, String) {
        let d = e.details().cloned().unwrap_or_default();
        (
            d["field"].as_str().unwrap_or_default().to_owned(),
            d["value"].as_str().unwrap_or_default().to_owned(),
        )
    }

    #[test]
    fn contract_vectors() -> Result<()> {
        let k = Some("Asia/Kolkata".to_owned());
        assert_eq!(
            at("2026-09-27T18:00", None)?,
            Resolved {
                at: utc("2026-09-27T12:30:00Z"),
                tz: k.clone()
            }
        );
        assert_eq!(
            at("2026-09-27 18:00", None)?,
            Resolved {
                at: utc("2026-09-27T12:30:00Z"),
                tz: k.clone()
            }
        );
        assert_eq!(
            at("18:00", None)?,
            Resolved {
                at: utc("2026-09-27T12:30:00Z"),
                tz: k.clone()
            }
        );
        assert_eq!(
            at("2026-09-27t18:00", None)?.at,
            utc("2026-09-27T12:30:00Z"),
            "t is case-insensitive"
        );
        assert_eq!(
            at("2026-09-27T18:00Z", None)?,
            Resolved {
                at: utc("2026-09-27T18:00:00Z"),
                tz: None
            }
        );
        assert_eq!(
            at("2026-09-27T18:00z", None)?.at,
            utc("2026-09-27T18:00:00Z")
        );
        assert_eq!(
            at("2026-09-27T18:00+05:30", None)?,
            Resolved {
                at: utc("2026-09-27T12:30:00Z"),
                tz: None
            }
        );
        assert_eq!(
            at("2026-09-27T18:00-0700", None)?.at,
            utc("2026-09-28T01:00:00Z")
        );
        assert_eq!(
            at("2026-09-27T18:00+02", None)?.at,
            utc("2026-09-27T16:00:00Z")
        );
        assert_eq!(
            at("2026-09-27T18:00:30.5", None)?.at,
            utc("2026-09-27T12:30:30.500Z")
        );
        let berlin = parse_tz("Europe/Berlin")?;
        assert_eq!(
            at("2026-09-27T18:00", Some(&berlin))?,
            Resolved {
                at: utc("2026-09-27T16:00:00Z"),
                tz: Some("Europe/Berlin".to_owned())
            }
        );
        // An offset in the value wins over --tz.
        assert_eq!(
            at("2026-09-27T18:00Z", Some(&berlin))?,
            Resolved {
                at: utc("2026-09-27T18:00:00Z"),
                tz: None
            }
        );
        Ok(())
    }

    #[test]
    fn past_values_say_when_and_suggest_tomorrow() {
        let e = at("11:00", None)
            .err()
            .unwrap_or_else(|| Error::internal("accepted"));
        assert_eq!(
            e.message(),
            "`--at 11:00` is in the past: 2026-09-27 11:00 IST (now 2026-09-27 11:42 IST)"
        );
        assert_eq!(e.hint(), Some("for tomorrow use --at 2026-09-28T11:00"));
        let d = e.details().cloned().unwrap_or_default();
        assert_eq!(d["field"], "--at");
        assert_eq!(d["value"], "11:00");
        assert_eq!(d["resolved"], "2026-09-27T05:30:00.000Z");
        assert_eq!(d["now"], "2026-09-27T06:12:00.000Z");
        let e = resolve("--expires-at", "2026-09-26T10:00Z", None, &kolkata(), now()).err();
        assert!(e.is_some_and(|e| {
            e.message()
                .starts_with("`--expires-at 2026-09-26T10:00Z` is in the past")
                && e.hint()
                    .is_some_and(|h| h.starts_with("give a time after now"))
        }));
        assert!(
            at("2026-09-27T11:42", None).is_err(),
            "exactly now is not the future"
        );
    }

    #[test]
    fn dst_gap_and_fold() -> Result<()> {
        let ny = parse_tz("America/New_York")?;
        let e = at("2026-03-08T02:30", Some(&ny)).err();
        // 2026-03-08 is before `now` too, so resolve against an earlier now.
        assert!(e.is_some());
        let early = utc("2026-01-01T00:00:00Z");
        let e = resolve("--at", "2026-03-08T02:30", Some(&ny), &kolkata(), early)
            .err()
            .unwrap_or_else(|| Error::internal("accepted"));
        assert_eq!(
            e.message(),
            "`--at 2026-03-08T02:30` does not exist in America/New_York (clocks skip that hour)"
        );
        assert_eq!(
            e.details().cloned().unwrap_or_default()["reason"],
            "nonexistent_local_time"
        );
        let fold = resolve("--at", "2026-11-01T01:30", Some(&ny), &kolkata(), now())?;
        assert_eq!(
            fold.at,
            utc("2026-11-01T05:30:00Z"),
            "the earlier of the fold"
        );
        Ok(())
    }

    #[test]
    fn malformed_values_are_refused() {
        for raw in [
            "tomorrow",
            "6pm",
            "2026-09-27T25:00",
            "2026-13-01T10:00",
            "2026-09-27T18:00[Asia/Kolkata]",
            "1790000000",
            "",
            "18",
            "18:0",
            "18:00:5",
            "18:00:00.",
            "18:00:00.1234567890",
            "2026-09-27X18:00",
            "2026-9-27T18:00",
            "18:00+5",
            "18:00+05:3",
            "18:00+24:00",
            "2026-02-30T10:00",
        ] {
            let e = parse("--at", raw).err();
            assert!(
                e.as_ref().is_some_and(|e| field_and_value(e)
                    == ("--at".to_owned(), raw.to_owned())
                    && e.hint().is_some()),
                "{raw:?}: {e:?}"
            );
        }
        let e = parse("--at", "6pm").err();
        assert!(e.is_some_and(|e| e.message()
            == "`--at 6pm` is not a date-time; use 2026-09-27T18:00, \"2026-09-27 18:00\", 18:00, or add Z / +05:30"));
        let e = parse("--at", "2026-09-27").err();
        assert!(e.is_some_and(
            |e| e.message() == "`--at 2026-09-27` has no time; add one, e.g. 2026-09-27T09:00"
        ));
        assert!(
            parse("--at", " 18:00 ").is_ok(),
            "surrounding whitespace is trimmed"
        );
        assert!(parse("--at", "18:00:59.123456789").is_ok());
    }

    #[test]
    fn time_zones() {
        assert!(parse_tz("Asia/Kolkata").is_ok());
        assert!(parse_tz("UTC").is_ok());
        let e = parse_tz("Asia/Kolkatta").err();
        assert!(e.is_some_and(
            |e| e.message() == "`--tz Asia/Kolkatta` is not an IANA time zone"
                && e.hint() == Some("e.g. Asia/Kolkata, Europe/Berlin, America/New_York, UTC")
                && e.details().is_some_and(|d| d["field"] == "--tz")
        ));
        assert!(parse_tz("../../etc/passwd").is_err());
        assert!(!needs_local("18:00Z", None));
        assert!(needs_local("18:00", None));
        let berlin = parse_tz("Europe/Berlin").ok();
        assert!(!needs_local("18:00", berlin.as_ref()));
    }

    #[cfg(unix)]
    #[test]
    fn the_mac_zone_comes_from_the_localtime_link() -> Result<()> {
        let dir = tempfile::tempdir().map_err(|e| Error::internal(e.to_string()))?;
        let link = dir.path().join("localtime");
        std::os::unix::fs::symlink("/var/db/timezone/zoneinfo/Asia/Kolkata", &link)
            .map_err(|e| Error::internal(e.to_string()))?;
        let z = local_zone_from(&link);
        assert_eq!(z.name.as_deref(), Some("Asia/Kolkata"));
        assert!(!z.fallback_utc);
        let missing = local_zone_from(&dir.path().join("nope"));
        assert!(missing.fallback_utc);
        assert!(missing.name.is_none());
        let bogus = dir.path().join("bogus");
        std::fs::write(&bogus, b"not tzif").map_err(|e| Error::internal(e.to_string()))?;
        assert!(local_zone_from(&bogus).fallback_utc);
        Ok(())
    }

    #[test]
    fn rendering() {
        let z = kolkata();
        let t = utc("2026-09-27T12:30:00Z");
        assert_eq!(render_short(t, &z.zone), "2026-09-27 18:00 IST");
        assert_eq!(
            render_local(t, &z.zone, z.name.as_deref()),
            "2026-09-27 18:00 IST (Asia/Kolkata)"
        );
        assert_eq!(
            render_short(utc("2026-09-27T12:30:15Z"), &z.zone),
            "2026-09-27 18:00:15 IST"
        );
        assert_eq!(
            render_near(utc("2026-09-27T13:00:00Z"), t, &z.zone),
            "18:30"
        );
        assert_eq!(
            render_near(utc("2026-09-28T13:00:00Z"), t, &z.zone),
            "2026-09-28 18:30 IST"
        );
        assert_eq!(
            render_relative(utc("2026-09-27T08:25:00Z"), now()),
            "in 2h 13m"
        );
        assert_eq!(
            render_relative(utc("2026-09-27T06:12:45Z"), now()),
            "in 45s"
        );
        assert_eq!(
            render_relative(utc("2026-09-27T23:12:00Z"), now()),
            "in 17h"
        );
        assert_eq!(
            render_relative(utc("2026-09-27T06:21:48Z"), now()),
            "in 9m 48s"
        );
        assert_eq!(
            render_relative(utc("2026-09-28T08:12:00Z"), now()),
            "in 1d 2h"
        );
        assert_eq!(
            render_relative(utc("2026-09-27T06:07:00Z"), now()),
            "5m ago"
        );
        assert_eq!(render_relative(now(), now()), "in 0s");
        assert_eq!(age(12_400), "12s");
        assert_eq!(age(4 * 60_000 + 59_000), "4m");
        assert_eq!(age(2 * 3_600_000), "2h");
        assert_eq!(age(3 * 86_400_000 + 5), "3d");
    }
}
