use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use jiff::civil::{Date, DateTime, Time};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Span, Timestamp, ZonedDateTime};
use regex::Regex;

use crate::util::english_date::{parse_date_string, Dialect};

static DATE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    // The date and time parts may be separated by a space or an ISO 8601 `T`
    Regex::new("^([0-9]{4})(-|:)([0-9]{1,2})(-|:)([0-9]{1,2})(?:[ Tt]([0-9]{1,2})(?::([0-9]{1,2})(?::([0-9]{1,2}))?)?)?$").unwrap()
});

static EXPLICIT_TIME_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b[0-9]{1,2}\s*[:.]\s*[0-9]{2}\b|\b[0-9]{1,2}\s*[ap]m\b").unwrap()
});

static TWELVE_OCLOCK_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(^|[^0-9:.])12(?::([0-9]{2})(?::([0-9]{2}))?)?\s*([ap])m\b").unwrap()
});

static TRAILING_TIME_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(.*\S)\s+([0-9]{1,2})(?::([0-9]{2})(?::([0-9]{2}))?)?\s*([ap]m)?$").unwrap()
});

static LOCAL_TIME_ZONE: LazyLock<TimeZone> = LazyLock::new(TimeZone::system);

pub fn local_time_zone() -> &'static TimeZone {
    &LOCAL_TIME_ZONE
}

pub fn local_now() -> ZonedDateTime {
    Timestamp::now().to_zoned_datetime(local_time_zone().clone())
}

fn floor_seconds(duration: SignedDuration) -> i64 {
    duration.as_secs() - i64::from(duration.subsec_nanos() < 0)
}

pub fn timestamp_seconds(timestamp: Timestamp) -> i64 {
    floor_seconds(timestamp.as_duration())
}

pub fn naive_seconds(dt: DateTime) -> i64 {
    floor_seconds(dt.duration_since(DateTime::constant(1970, 1, 1, 0, 0, 0, 0)))
}

fn whole_day(date: Date) -> (DateTime, DateTime) {
    (date.at(0, 0, 0, 0), date.at(23, 59, 59, 0))
}

fn trailing_time(caps: &regex::Captures) -> Option<Time> {
    let mut hour: i8 = caps[2].parse().ok()?;
    if let Some(meridiem) = caps.get(5) {
        if hour == 0 || hour > 12 {
            return None;
        }
        hour %= 12;
        if meridiem.as_str().eq_ignore_ascii_case("pm") {
            hour += 12;
        }
    }
    let minute = caps.get(3).map_or(Some(0), |m| m.as_str().parse().ok())?;
    let second = caps.get(4).map_or(Some(0), |m| m.as_str().parse().ok())?;
    Time::new(hour, minute, second, 0).ok()
}

fn parse_natural_datetime(s: &str, dialect: Dialect) -> Option<DateTime> {
    let now = local_now();
    if let Some(caps) = TRAILING_TIME_REGEX.captures(s)
        && (caps.get(3).is_some() || caps.get(5).is_some())
    {
        let time = trailing_time(&caps)?;
        let date_time = parse_date_string(&caps[1], &now, dialect)?;
        return Some(date_time.date().to_datetime(time));
    }
    parse_date_string(s, &now, dialect)
}

fn normalize_twelve_oclock(s: &str) -> std::borrow::Cow<'_, str> {
    TWELVE_OCLOCK_REGEX.replace_all(s, |caps: &regex::Captures| {
        let before = &caps[1];
        let hour = if caps[4].eq_ignore_ascii_case("a") { "0" } else { "12" };
        let minutes = caps.get(2).map_or("00", |m| m.as_str());
        match caps.get(3) {
            Some(seconds) => format!("{before}{hour}:{minutes}:{}", seconds.as_str()),
            None => format!("{before}{hour}:{minutes}"),
        }
    })
}

static US_DATES: Mutex<bool> = Mutex::new(false);

pub fn set_us_dates(us: bool) {
    *US_DATES.lock().unwrap() = us;
}

#[cfg(test)]
thread_local! {
    static DATETIME_PARSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub fn datetime_parses() -> usize {
    DATETIME_PARSES.with(|parses| parses.get())
}

pub fn parse_datetime(s: &str) -> Result<(DateTime, DateTime), String> {
    #[cfg(test)]
    DATETIME_PARSES.with(|parses| parses.set(parses.get() + 1));
    let error = || "Error parsing date/time value: ".to_string() + s;

    if s == "today" {
        return Ok(whole_day(local_now().date()));
    }

    if s == "yesterday" {
        return local_now().date().yesterday().map(whole_day).map_err(|_| error());
    }

    match DATE_REGEX.captures(s) {
        Some(cap) => {
            // Ensure consistent separator (both dashes or both colons)
            if cap[2] != cap[4] {
                return Err(error());
            }
            let part = |index: usize| -> Result<i8, String> { cap[index].parse().map_err(|_| error()) };
            let optional_part = |index: usize, finish_default: i8| -> Result<(i8, i8), String> {
                match cap.get(index) {
                    Some(_) => part(index).map(|value| (value, value)),
                    None => Ok((0, finish_default)),
                }
            };
            let year: i16 = cap[1].parse().map_err(|_| error())?;
            let month = part(3)?;
            let day = part(5)?;
            let (hour_start, hour_finish) = optional_part(6, 23)?;
            let (min_start, min_finish) = optional_part(7, 59)?;
            let (sec_start, sec_finish) = optional_part(8, 59)?;

            // Build the civil datetime directly: a round-trip through the local
            // time zone would trip on DST-skipped or ambiguous midnights while
            // adding nothing to the result
            let date = Date::new(year, month, day).map_err(|_| error())?;
            let start = Time::new(hour_start, min_start, sec_start, 0).map_err(|_| error())?;
            let finish = Time::new(hour_finish, min_finish, sec_finish, 0).map_err(|_| error())?;

            Ok((date.to_datetime(start), date.to_datetime(finish)))
        }
        None => {
            // Simplified relative-day syntax: any length of `+N`/`-N` counts
            // as days from today; longer non-numeric strings fall through to
            // the natural-language parser.
            if s.len() >= 2
                && (s.starts_with("+") || s.starts_with("-"))
                && let Ok(days) = s.parse::<i64>()
            {
                Span::new()
                    .try_days(days)
                    .and_then(|offset| local_now().date().checked_add(offset))
                    .map(whole_day)
                    .map_err(|_| error())
            } else if s.len() >= 5 {
                let dialect = match *US_DATES.lock().unwrap() {
                    true => Dialect::Us,
                    false => Dialect::Uk,
                };
                let normalized = normalize_twelve_oclock(s);
                match parse_natural_datetime(&normalized, dialect) {
                    Some(date_time) => {
                        let finish = if date_time.hour() == 0
                            && date_time.minute() == 0
                            && date_time.second() == 0
                            && !EXPLICIT_TIME_REGEX.is_match(&normalized)
                        {
                            date_time.date().at(23, 59, 59, date_time.subsec_nanosecond())
                        } else {
                            date_time
                        };

                        Ok((date_time, finish))
                    }
                    None => Err(error()),
                }
            } else {
                Err(error())
            }
        }
    }
}

pub fn system_time_to_unix(sdt: SystemTime) -> Option<(i64, u32)> {
    match sdt.duration_since(UNIX_EPOCH) {
        Ok(dur) => Some((i64::try_from(dur.as_secs()).ok()?, dur.subsec_nanos())),
        Err(e) => {
            let dur = e.duration();
            let secs = i64::try_from(dur.as_secs()).ok()?;
            if dur.subsec_nanos() == 0 {
                Some((secs.checked_neg()?, 0))
            } else {
                Some((secs.checked_neg()?.checked_sub(1)?, 1_000_000_000 - dur.subsec_nanos()))
            }
        }
    }
}

pub fn system_time_to_naive_local(sdt: SystemTime) -> Option<DateTime> {
    let (sec, nsec) = system_time_to_unix(sdt)?;

    Timestamp::new(sec, nsec as i32)
        .ok()
        .map(|timestamp| local_time_zone().to_datetime(timestamp))
}

pub fn to_local_datetime(dt: &zip::DateTime) -> DateTime {
    let year = dt.year() as i16;
    let date = Date::new(year, dt.month() as i8, dt.day() as i8)
        .or_else(|_| Date::new(year, 1, 1))
        .unwrap_or(Date::constant(1970, 1, 1));
    let time = Time::new(dt.hour() as i8, dt.minute() as i8, dt.second() as i8, 0)
        .unwrap_or(Time::midnight());
    date.to_datetime(time)
}

pub fn format_datetime(dt: &DateTime) -> String {
    dt.strftime("%Y-%m-%d %H:%M:%S").to_string()
}

pub fn format_date(date: &Date) -> String {
    date.strftime("%Y-%m-%d").to_string()
}

pub fn format_time(time: &Time) -> String {
    time.strftime("%H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration as StdDuration;
    use jiff::civil::date;
    use jiff::ToSpan;

    #[test]
    fn test_parse_today() {
        let result = parse_datetime("today").unwrap();
        let now = local_now().date();
        let start = now.at(0, 0, 0, 0);
        let finish = now.at(23, 59, 59, 0);

        assert_eq!(result.0, start);
        assert_eq!(result.1, finish);
    }

    #[test]
    fn test_parse_yesterday() {
        let result = parse_datetime("yesterday").unwrap();
        let yesterday = local_now().date() - 1.days();
        let start = yesterday.at(0, 0, 0, 0);
        let finish = yesterday.at(23, 59, 59, 0);

        assert_eq!(result.0, start);
        assert_eq!(result.1, finish);
    }

    #[test]
    fn test_parse_two_days_ago() {
        let result = parse_datetime("2 days ago 00:00").unwrap();
        let two_days_ago = local_now().date() - 2.days();
        let start = two_days_ago.at(0, 0, 0, 0);

        assert_eq!(result.0, start);
        assert_eq!(result.1, start);
    }

    #[test]
    fn relative_days_keep_an_explicit_trailing_time() {
        let yesterday = local_now().date() - 1.days();
        let at = |h, m| yesterday.at(h, m, 0, 0);

        assert_eq!(parse_datetime("yesterday 15:30").unwrap(), (at(15, 30), at(15, 30)));
        assert_eq!(parse_datetime("yesterday 3pm").unwrap(), (at(15, 0), at(15, 0)));
        assert_eq!(parse_datetime("yesterday 12am").unwrap(), (at(0, 0), at(0, 0)));
        assert_eq!(parse_datetime("1 day ago 00:00").unwrap(), (at(0, 0), at(0, 0)));
        assert!(parse_datetime("yesterday 25:00").is_err());
        assert!(parse_datetime("yesterday 13pm").is_err());
    }

    #[test]
    fn test_parse_two_days_ago_simplified() {
        let result = parse_datetime("-2").unwrap();
        let two_days_ago = local_now().date() - 2.days();
        let start = two_days_ago.at(0, 0, 0, 0);
        let finish = two_days_ago.at(23, 59, 59, 0);

        assert_eq!(result.0, start);
        assert_eq!(result.1, finish);
    }

    #[test]
    fn test_parse_specific_date() {
        let result = parse_datetime("2023-12-11").unwrap();
        let date = date(2023, 12, 11);
        let start = date.at(0, 0, 0, 0);
        let finish = date.at(23, 59, 59, 0);

        assert_eq!(result.0, start);
        assert_eq!(result.1, finish);
    }

    #[test]
    fn test_parse_specific_datetime() {
        let result = parse_datetime("2023-12-11 14:30:45").unwrap();
        let date = date(2023, 12, 11);
        let start = date.at(14, 30, 45, 0);
        let finish = start;

        assert_eq!(result.0, start);
        assert_eq!(result.1, finish);
    }

    #[test]
    fn test_invalid_format() {
        let result = parse_datetime("invalid-date");

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Error parsing date/time value: invalid-date");
    }

    #[test]
    fn test_parse_invalid_calendar_date() {
        // Genuinely invalid dates must still error after dropping the Local round-trip
        assert!(parse_datetime("2023-02-30").is_err());
        assert!(parse_datetime("2023-13-01").is_err());
        assert!(parse_datetime("2023-04-31").is_err());
    }

    #[test]
    fn test_parse_out_of_range_time() {
        assert!(parse_datetime("2024-01-01 25:00:00").is_err());
        assert!(parse_datetime("2024-01-01 12:61:00").is_err());
        assert!(parse_datetime("2024-01-01 12:00:99").is_err());
    }

    #[test]
    fn natural_language_dates_span_the_day_only_without_an_explicit_time() {
        let midnight = date(2024, 6, 1).at(0, 0, 0, 0);
        let end_of_day = date(2024, 6, 1).at(23, 59, 59, 0);

        assert_eq!(parse_datetime("1 June 2024").unwrap(), (midnight, end_of_day));
        assert_eq!(parse_datetime("1 June 2024 00:00:00").unwrap(), (midnight, midnight));
        assert_eq!(parse_datetime("1 June 2024 00:00").unwrap(), (midnight, midnight));
        let noon = date(2024, 6, 1).at(12, 0, 0, 0);
        assert_eq!(parse_datetime("1 June 2024 12:00").unwrap(), (noon, noon));
    }

    #[test]
    fn twelve_oclock_follows_the_twelve_hour_clock() {
        let at = |h, m, s| date(2024, 6, 1).at(h, m, s, 0);

        assert_eq!(parse_datetime("1 June 2024 12 am").unwrap(), (at(0, 0, 0), at(0, 0, 0)));
        assert_eq!(parse_datetime("1 June 2024 12am").unwrap(), (at(0, 0, 0), at(0, 0, 0)));
        assert_eq!(parse_datetime("1 June 2024 12 pm").unwrap(), (at(12, 0, 0), at(12, 0, 0)));
        assert_eq!(parse_datetime("1 June 2024 12:30 am").unwrap(), (at(0, 30, 0), at(0, 30, 0)));
        assert_eq!(parse_datetime("1 June 2024 12:30:15 PM").unwrap(), (at(12, 30, 15), at(12, 30, 15)));
        assert_eq!(parse_datetime("1 June 2024 1 pm").unwrap(), (at(13, 0, 0), at(13, 0, 0)));
        assert_eq!(parse_datetime("1 June 2024 1 am").unwrap(), (at(1, 0, 0), at(1, 0, 0)));
        assert_eq!(parse_datetime("1 June 2024 1:12 am").unwrap(), (at(1, 12, 0), at(1, 12, 0)));
        assert_eq!(parse_datetime("1 June 2024 1:12 pm").unwrap(), (at(13, 12, 0), at(13, 12, 0)));
        assert_eq!(parse_datetime("1 June 2024 1:15:12 pm").unwrap(), (at(13, 15, 12), at(13, 15, 12)));
        assert_eq!(parse_datetime("1 June 2024 12:12 am").unwrap(), (at(0, 12, 0), at(0, 12, 0)));
        assert_eq!(parse_datetime("1 June 2024 12:12:12 pm").unwrap(), (at(12, 12, 12), at(12, 12, 12)));
    }

    #[test]
    fn test_parse_non_numeric_plus_minus() {
        assert!(parse_datetime("+abc").is_err());
        assert!(parse_datetime("-xyz").is_err());
    }

    #[test]
    fn test_to_local_datetime_feb() {
        // Regression: previously panicked when current day > days in target month
        let dt = zip::DateTime::from_date_and_time(2023, 2, 15, 10, 30, 0).unwrap();
        let result = to_local_datetime(&dt);
        assert_eq!(result.year(), 2023);
        assert_eq!(result.month(), 2);
        assert_eq!(result.day(), 15);
        assert_eq!(result.hour(), 10);
        assert_eq!(result.minute(), 30);
    }

    #[test]
    fn test_partial_date_parsing() {
        let result = parse_datetime("2023-12-11 14:30").unwrap();
        let date = date(2023, 12, 11);
        let start = date.at(14, 30, 0, 0);
        let finish = date.at(14, 30, 59, 0);

        assert_eq!(result.0, start);
        assert_eq!(result.1, finish);
    }

    #[test]
    fn iso_t_separator_matches_the_same_range_as_the_space_form() {
        let date = date(2026, 10, 9);

        let minute = parse_datetime("2026-10-09T10:09").unwrap();
        assert_eq!(minute, parse_datetime("2026-10-09 10:09").unwrap());
        assert_eq!(minute.0, date.at(10, 9, 0, 0));
        assert_eq!(minute.1, date.at(10, 9, 59, 0));

        let hour = parse_datetime("2026-10-09T10").unwrap();
        assert_eq!(hour.0, date.at(10, 0, 0, 0));
        assert_eq!(hour.1, date.at(10, 59, 59, 0));

        let second = parse_datetime("2026-10-09T10:09:05").unwrap();
        assert_eq!(second.0, date.at(10, 9, 5, 0));
        assert_eq!(second.1, second.0);

        let lowercase = parse_datetime("2026-10-09t10:09").unwrap();
        assert_eq!(lowercase, minute);
    }

    #[test]
    fn test_substring_date_should_not_match() {
        // Date regex should not match dates embedded in other text
        assert!(parse_datetime("abc2024-01-15xyz").is_err());
    }

    #[test]
    fn test_mixed_separators_should_not_match() {
        // Mixed dash and colon separators should not be accepted
        assert!(parse_datetime("2024-01:15").is_err());
    }

    #[test]
    fn test_colon_separator_date_accepted() {
        // EXIF-style colon-separated dates should work
        let result = parse_datetime("2024:01:15");
        assert!(result.is_ok());
    }

    #[test]
    fn test_system_time_to_naive_local_unix_epoch() {
        let result = system_time_to_naive_local(UNIX_EPOCH);
        assert!(result.is_some(), "UNIX_EPOCH must convert successfully");

        let expected = local_time_zone().to_datetime(Timestamp::UNIX_EPOCH);
        assert_eq!(result.unwrap(), expected);
    }

    #[test]
    fn test_system_time_to_naive_local_now_roundtrip() {
        let now = SystemTime::now();
        let result = system_time_to_naive_local(now);
        assert!(result.is_some(), "SystemTime::now() must convert");

        let now_local = local_now().datetime();
        let diff = now_local.duration_since(result.unwrap()).as_secs().abs();
        assert!(diff < 5, "round-trip drift should be tiny, got {}s", diff);
    }

    #[test]
    fn test_system_time_to_naive_local_before_epoch() {
        let one_sec_before = UNIX_EPOCH - StdDuration::from_secs(1);
        let result = system_time_to_naive_local(one_sec_before);
        assert!(result.is_some(), "pre-epoch SystemTime must convert");

        let expected = local_time_zone().to_datetime(Timestamp::from_second(-1).unwrap());
        assert_eq!(result.unwrap(), expected);
    }

    #[test]
    fn test_system_time_to_naive_local_before_epoch_with_nanos() {
        let pre_epoch = UNIX_EPOCH - StdDuration::new(1, 0) + StdDuration::from_millis(500);
        let result = system_time_to_naive_local(pre_epoch);
        assert!(result.is_some(), "pre-epoch SystemTime with nanos must convert");
    }

    #[test]
    fn test_system_time_to_naive_local_far_future_does_not_panic() {
        let huge = StdDuration::from_secs(u64::MAX / 2);
        if let Some(t) = SystemTime::now().checked_add(huge) {
            let _ = system_time_to_naive_local(t);
        }
    }

    #[test]
    fn test_system_time_to_naive_local_max_does_not_panic() {
        let mut times = vec![];
        if let Some(t) = SystemTime::now().checked_add(StdDuration::from_secs(i64::MAX as u64 - 1)) {
            times.push(t);
        }
        if let Some(t) = UNIX_EPOCH.checked_add(StdDuration::from_secs(100_000_000_000_000)) {
            times.push(t);
        }
        for t in times {
            let _ = system_time_to_naive_local(t);
        }
    }

    #[test]
    fn relative_day_offsets_beyond_the_calendar_are_errors() {
        for s in ["-100000000", "+100000000", "+99999999999999999", "-9223372036854775808", "+9223372036854775807"] {
            let result = parse_datetime(s);
            assert!(result.is_err(), "{} should be rejected, got {:?}", s, result);
            assert_eq!(result.unwrap_err(), format!("Error parsing date/time value: {}", s));
        }
        for s in ["-1", "+0", "+3", "-36500"] {
            assert!(parse_datetime(s).is_ok(), "{} should still parse", s);
        }
    }

    #[test]
    fn non_ascii_digits_are_rejected_without_panicking() {
        for s in [
            "\u{662}\u{660}\u{662}\u{666}-\u{661}\u{660}-\u{661}\u{665}",
            "\u{FF12}\u{FF10}\u{FF12}\u{FF16}-\u{FF11}\u{FF10}-\u{FF11}\u{FF15}",
            "2026-\u{661}\u{660}-15",
            "2026-10-15 \u{661}\u{662}:00",
            "2026-10-15 12:\u{663}\u{660}:\u{660}\u{660}",
        ] {
            let result = parse_datetime(s);
            assert!(result.is_err(), "{:?} should be rejected, got {:?}", s, result);
        }
        assert!(parse_datetime("2026-10-15").is_ok());
        assert!(parse_datetime("2026-10-15 12:30:00").is_ok());
    }

    #[test]
    fn test_system_time_to_naive_local_wrapped_seconds_returns_none() {
        let beyond_i64 = StdDuration::from_secs(u64::MAX - 10);
        if let Some(t) = UNIX_EPOCH.checked_add(beyond_i64) {
            let result = system_time_to_naive_local(t);
            assert!(
                result.is_none(),
                "values past i64::MAX seconds must yield None, got {:?}",
                result
            );
        }
    }
}
