//! UTC timestamps for saved content, release manifests, and publication schedules.

use std::{
    fmt,
    ops::{Add, AddAssign, Sub},
    str::FromStr,
    time::SystemTime,
};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::{Date, Month, OffsetDateTime, SignedDuration, UtcOffset};

const BILLION: u32 = 1_000_000_000;

/// A UTC instant that retains leap seconds and nanosecond precision in portable data.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Timestamp {
    seconds: i64,
    nanos: u32,
}

#[derive(Debug, thiserror::Error)]
#[error("invalid timestamp")]
pub struct InvalidTimestamp;

impl Timestamp {
    #[must_use]
    pub fn now() -> Self {
        SystemTime::now().into()
    }

    pub fn from_rfc3339(value: &str) -> Result<Self, InvalidTimestamp> {
        let bytes = value.as_bytes();
        if bytes.len() < 20
            || bytes.get(4) != Some(&b'-')
            || bytes.get(7) != Some(&b'-')
            || !matches!(bytes.get(10), Some(b'T' | b't' | b' '))
            || bytes.get(13) != Some(&b':')
            || bytes.get(16) != Some(&b':')
            || ![0..4, 5..7, 8..10, 11..13, 14..16, 17..19]
                .iter()
                .all(|part| bytes[part.clone()].iter().all(u8::is_ascii_digit))
        {
            return Err(InvalidTimestamp);
        }
        let suffix = value.get(19..).ok_or(InvalidTimestamp)?;
        let zone = if let Some(fraction) = suffix.strip_prefix('.') {
            let zone = fraction.trim_start_matches(|c: char| c.is_ascii_digit());
            if zone.len() == fraction.len() {
                return Err(InvalidTimestamp);
            }
            zone
        } else {
            suffix
        };
        if !zone.eq_ignore_ascii_case("Z")
            && !zone.strip_prefix(['+', '-', '−']).is_some_and(|offset| {
                offset.len() == 5
                    && offset.as_bytes()[2] == b':'
                    && offset.as_bytes()[..2].iter().all(u8::is_ascii_digit)
                    && offset.as_bytes()[3..].iter().all(u8::is_ascii_digit)
            })
        {
            return Err(InvalidTimestamp);
        }
        value.parse()
    }

    pub(crate) fn editor_utc(value: &str) -> Option<Self> {
        let (date, clock) = value.split_once('T')?;
        if !clock
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b':' || b.is_ascii_whitespace())
        {
            return None;
        }
        let seconds = if clock.matches(':').count() == 1 {
            ":00"
        } else {
            ""
        };
        parse_instant(&format!("{date}T{clock}{seconds}Z"))
    }

    #[must_use]
    pub fn from_unix_parts(seconds: i64, nanos: u32) -> Option<Self> {
        let date = OffsetDateTime::from_unix_timestamp(seconds).ok()?;
        let leap = date.second() == 59 && nanos < 2 * BILLION;
        ((-262_143..=262_142).contains(&date.year()) && (nanos < BILLION || leap))
            .then_some(Self { seconds, nanos })
    }

    #[must_use]
    pub fn from_utc_components(
        year: i32,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
        second: u8,
    ) -> Option<Self> {
        let date = Date::from_calendar_date(year, Month::try_from(month).ok()?, day).ok()?;
        let time = date
            .with_hms(hour, minute, second.min(59))
            .ok()?
            .assume_utc();
        let nanos = match second {
            0..=59 => 0,
            60 => BILLION,
            _ => return None,
        };
        Self::from_unix_parts(time.unix_timestamp(), nanos)
    }

    #[must_use]
    pub const fn unix_seconds(self) -> i64 {
        self.seconds
    }

    pub(crate) fn civil(self, offset: UtcOffset) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(self.seconds)
            .expect("a timestamp has a valid calendar date")
            .to_offset(offset)
    }

    fn second(self, local: OffsetDateTime) -> u8 {
        local.second() + u8::from(self.nanos >= BILLION)
    }

    fn fraction(self) -> String {
        let nanos = self.nanos % BILLION;
        if nanos == 0 {
            String::new()
        } else if nanos.is_multiple_of(1_000_000) {
            format!(".{:03}", nanos / 1_000_000)
        } else if nanos.is_multiple_of(1_000) {
            format!(".{:06}", nanos / 1_000)
        } else {
            format!(".{nanos:09}")
        }
    }

    fn clock_text(self, local: OffsetDateTime) -> String {
        let hour = local.hour();
        let minute = local.minute();
        let second = self.second(local);
        format!("{hour:02}:{minute:02}:{second:02}")
    }

    pub(crate) fn seconds_at(self, offset: UtcOffset) -> String {
        let local = self.civil(offset);
        let zone = if offset.is_utc() {
            "Z".to_owned()
        } else {
            let minutes = (offset.whole_seconds().unsigned_abs() + 30) / 60;
            let sign = if offset.is_negative() { '-' } else { '+' };
            format!("{sign}{:02}:{:02}", minutes / 60, minutes % 60)
        };
        format!("{}T{}{zone}", date_text(local), self.clock_text(local))
    }

    #[must_use]
    pub fn rfc3339_seconds(self) -> String {
        self.seconds_at(UtcOffset::UTC)
    }

    fn utc_text(self, suffix: &str) -> String {
        let utc = self.civil(UtcOffset::UTC);
        format!(
            "{}T{}{}{suffix}",
            date_text(utc),
            self.clock_text(utc),
            self.fraction()
        )
    }

    #[must_use]
    pub fn sqlite_text(self) -> String {
        self.utc_text("+00:00")
    }

    pub(crate) fn compact_minute(self) -> String {
        let utc = self.civil(UtcOffset::UTC);
        let day = format!(
            "{}{:02}{:02}",
            year_text(utc.year()),
            u8::from(utc.month()),
            utc.day()
        );
        format!("{day}-{:02}{:02}", utc.hour(), utc.minute())
    }

    pub(crate) fn compact_second(self) -> String {
        format!(
            "{}{:02}",
            self.compact_minute(),
            self.second(self.civil(UtcOffset::UTC))
        )
    }

    pub(crate) fn editor_minute(self) -> String {
        let utc = self.civil(UtcOffset::UTC);
        format!("{}T{:02}:{:02}", date_text(utc), utc.hour(), utc.minute())
    }
}

fn year_text(year: i32) -> String {
    if (0..=9999).contains(&year) {
        format!("{year:04}")
    } else {
        format!("{year:+05}")
    }
}

pub(crate) fn date_text(date: OffsetDateTime) -> String {
    format!(
        "{}-{:02}-{:02}",
        year_text(date.year()),
        u8::from(date.month()),
        date.day()
    )
}

fn utc_offset(text: &str) -> Option<i32> {
    if text.eq_ignore_ascii_case("Z") || text.eq_ignore_ascii_case("UTC") {
        return Some(0);
    }
    let (digits, sign) = if let Some(digits) = text.strip_prefix('+') {
        (digits, 1)
    } else {
        (text.strip_prefix(['-', '−'])?, -1)
    };
    let (hour, rest) = digits.split_at_checked(2)?;
    let minute = rest.trim_start_matches(|c: char| c == ':' || c.is_whitespace());
    if minute.len() != 2
        || !hour
            .bytes()
            .chain(minute.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let hour: u8 = hour.parse().ok()?;
    let minute: u8 = minute.parse().ok()?;
    (hour < 24 && minute < 60).then(|| sign * (i32::from(hour) * 3600 + i32::from(minute) * 60))
}

fn digits<'a>(input: &mut &'a str, limit: usize) -> Option<&'a str> {
    let count = input
        .bytes()
        .take(limit)
        .take_while(u8::is_ascii_digit)
        .count();
    let (number, remaining) = input.split_at(count);
    *input = remaining;
    (!number.is_empty()).then_some(number)
}

fn component(input: &mut &str) -> Option<u8> {
    *input = input.trim_start();
    digits(input, 2)?.parse().ok()
}

fn parse_instant(text: &str) -> Option<Timestamp> {
    let mut input = text.trim();
    let signed = input.starts_with(['-', '+']);
    let negative = input.starts_with('-');
    if signed {
        input = &input[1..];
    }
    let year: i32 = digits(&mut input, if signed { usize::MAX } else { 4 })?
        .parse()
        .ok()?;
    input = input.trim_start().strip_prefix('-')?;
    let month = component(&mut input)?;
    input = input.trim_start().strip_prefix('-')?;
    let day = component(&mut input)?;
    input = input.strip_prefix(['T', 't', ' '])?;
    let hour = component(&mut input)?;
    input = input.trim_start().strip_prefix(':')?;
    let minute = component(&mut input)?;
    input = input.trim_start().strip_prefix(':')?;
    let second = component(&mut input)?;
    let fraction = if let Some(rest) = input.strip_prefix('.') {
        input = rest;
        digits(&mut input, usize::MAX)?
    } else {
        ""
    };
    let offset = utc_offset(input.trim_start())?;
    let nanos = fraction
        .bytes()
        .take(9)
        .chain(std::iter::repeat(b'0'))
        .take(9)
        .fold(0_u32, |n, digit| n * 10 + u32::from(digit - b'0'));
    let local = Timestamp::from_utc_components(
        if negative { -year } else { year },
        month,
        day,
        hour,
        minute,
        second,
    )?;
    Timestamp::from_unix_parts(local.seconds - i64::from(offset), local.nanos + nanos)
}

impl FromStr for Timestamp {
    type Err = InvalidTimestamp;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        parse_instant(text).ok_or(InvalidTimestamp)
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let utc = self.civil(UtcOffset::UTC);
        write!(
            f,
            "{} {}{} UTC",
            date_text(utc),
            self.clock_text(utc),
            self.fraction()
        )
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.utc_text("Z"))
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = std::borrow::Cow::<'de, str>::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

impl From<SystemTime> for Timestamp {
    fn from(value: SystemTime) -> Self {
        let utc = OffsetDateTime::from(value);
        Self::from_unix_parts(utc.unix_timestamp(), utc.nanosecond())
            .expect("system clock is within the supported date range")
    }
}

impl From<Timestamp> for SystemTime {
    fn from(value: Timestamp) -> Self {
        Self::from(value.civil(UtcOffset::UTC))
            + std::time::Duration::from_nanos(u64::from(value.nanos))
    }
}

impl Add<SignedDuration> for Timestamp {
    type Output = Self;
    fn add(self, duration: SignedDuration) -> Self {
        let billion = i128::from(BILLION);
        let mut nanos = i128::from(self.nanos % BILLION) + duration.whole_nanoseconds();
        if self.nanos >= BILLION {
            if (0..billion).contains(&nanos) {
                return Self {
                    seconds: self.seconds,
                    nanos: BILLION + u32::try_from(nanos).expect("fraction of one second"),
                };
            }
            if nanos < 0 {
                nanos += billion;
            }
        }
        let instant = i128::from(self.seconds) * billion + nanos;
        let utc = OffsetDateTime::from_unix_timestamp_nanos(instant)
            .expect("timestamp arithmetic exceeds the calendar range");
        Self::from_unix_parts(utc.unix_timestamp(), utc.nanosecond())
            .expect("timestamp arithmetic exceeds the supported date range")
    }
}

impl AddAssign<SignedDuration> for Timestamp {
    fn add_assign(&mut self, duration: SignedDuration) {
        *self = *self + duration;
    }
}

impl Sub<SignedDuration> for Timestamp {
    type Output = Self;
    fn sub(self, duration: SignedDuration) -> Self {
        self + -duration
    }
}

impl Sub for Timestamp {
    type Output = SignedDuration;
    fn sub(self, other: Self) -> SignedDuration {
        let mut seconds = self.seconds - other.seconds;
        let left = self.seconds.rem_euclid(86_400);
        let right = other.seconds.rem_euclid(86_400);
        if left > right && other.nanos >= BILLION {
            seconds += 1;
        }
        if left < right && self.nanos >= BILLION {
            seconds -= 1;
        }
        SignedDuration::seconds(seconds)
            + SignedDuration::nanoseconds(i64::from(self.nanos) - i64::from(other.nanos))
    }
}

/// An IANA time-zone identifier and its bundled transition rules.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SiteTimeZone {
    name: &'static str,
    rules: tz::TimeZoneRef<'static>,
}

impl SiteTimeZone {
    pub const UTC: Self = Self {
        name: "UTC",
        rules: tz::TimeZoneRef::utc(),
    };

    #[must_use]
    pub const fn name(self) -> &'static str {
        self.name
    }

    pub(crate) fn offset(self, date: Timestamp) -> UtcOffset {
        let local = self
            .rules
            .find_local_time_type(date.unix_seconds())
            .expect("bundled IANA rules cover the supported date range");
        UtcOffset::from_whole_seconds(local.ut_offset())
            .expect("bundled IANA offsets fit the clock range")
    }
}

impl FromStr for SiteTimeZone {
    type Err = InvalidTimeZone;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let rules = tzdb::tz_by_name(value).ok_or(InvalidTimeZone)?;
        let name = tzdb::TZ_NAMES
            .iter()
            .copied()
            .find(|name| *name == value)
            .ok_or(InvalidTimeZone)?;
        Ok(Self { name, rules })
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown IANA time zone")]
pub struct InvalidTimeZone;
