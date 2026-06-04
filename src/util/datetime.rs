use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Duration, Local, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Timelike, Utc};
use chrono_english::{parse_date_string, Dialect};
use regex::Regex;

static DATE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("^(\\d{4})(-|:)(\\d{1,2})(-|:)(\\d{1,2})(?: (\\d{1,2})(?::(\\d{1,2})(?::(\\d{1,2}))?)?)?$").unwrap()
});

static US_DATES: Mutex<bool> = Mutex::new(false);

pub fn set_us_dates(us: bool) {
    *US_DATES.lock().unwrap() = us;
}

pub fn parse_datetime(s: &str) -> Result<(NaiveDateTime, NaiveDateTime), String> {
    if s == "today" {
        let date = Local::now().date_naive();
        let start = date.and_hms_opt(0, 0, 0).unwrap();
        let finish = date.and_hms_opt(23, 59, 59).unwrap();

        return Ok((start, finish));
    }

    if s == "yesterday" {
        let date = Local::now().date_naive() - Duration::try_days(1).unwrap();
        let start = date.and_hms_opt(0, 0, 0).unwrap();
        let finish = date.and_hms_opt(23, 59, 59).unwrap();

        return Ok((start, finish));
    }

    match DATE_REGEX.captures(s) {
        Some(cap) => {
            // Ensure consistent separator (both dashes or both colons)
            if cap[2] != cap[4] {
                return Err("Error parsing date/time value: ".to_string() + s);
            }
            let year: i32 = cap[1].parse().unwrap();
            let month: u32 = cap[3].parse().unwrap();
            let day: u32 = cap[5].parse().unwrap();

            let hour_start: u32;
            let hour_finish: u32;
            match cap.get(6) {
                Some(val) => {
                    hour_start = val.as_str().parse().unwrap();
                    hour_finish = hour_start;
                }
                None => {
                    hour_start = 0;
                    hour_finish = 23;
                }
            }

            let min_start: u32;
            let min_finish: u32;
            match cap.get(7) {
                Some(val) => {
                    min_start = val.as_str().parse().unwrap();
                    min_finish = min_start;
                }
                None => {
                    min_start = 0;
                    min_finish = 59;
                }
            }

            let sec_start: u32;
            let sec_finish: u32;
            match cap.get(8) {
                Some(val) => {
                    sec_start = val.as_str().parse().unwrap();
                    sec_finish = sec_start;
                }
                None => {
                    sec_start = 0;
                    sec_finish = 59;
                }
            }

            match Local.with_ymd_and_hms(year, month, day, 0, 0, 0) {
                LocalResult::Single(date) => {
                    let base = date.naive_local();
                    let start = base
                        .with_hour(hour_start)
                        .and_then(|d| d.with_minute(min_start))
                        .and_then(|d| d.with_second(sec_start));
                    let finish = base
                        .with_hour(hour_finish)
                        .and_then(|d| d.with_minute(min_finish))
                        .and_then(|d| d.with_second(sec_finish));

                    match (start, finish) {
                        (Some(s), Some(f)) => Ok((s, f)),
                        _ => Err("Error parsing date/time value: ".to_string() + s),
                    }
                }
                _ => Err("Error converting date/time to local: ".to_string() + s),
            }
        }
        None => {
            if s.len() >= 5 {
                let dialect = match *US_DATES.lock().unwrap() {
                    true => Dialect::Us,
                    false => Dialect::Uk,
                };
                match parse_date_string(s, Local::now(), dialect) {
                    Ok(date_time) => {
                        let date_time = date_time.naive_local();
                        let finish = if date_time.hour() == 0
                            && date_time.minute() == 0
                            && date_time.second() == 0
                        {
                            date_time
                                .with_hour(23)
                                .unwrap()
                                .with_minute(59)
                                .unwrap()
                                .with_second(59)
                                .unwrap()
                        } else {
                            date_time
                        };

                        Ok((date_time, finish))
                    }
                    _ => Err("Error parsing date/time value: ".to_string() + s),
                }
            } else if s.len() >= 2 && (s.starts_with("+") || s.starts_with("-")) {
                match s.parse::<i64>() {
                    Ok(days) => {
                        let date = Local::now().date_naive() + Duration::days(days);
                        let start = date.and_hms_opt(0, 0, 0).unwrap();
                        let finish = date.and_hms_opt(23, 59, 59).unwrap();

                        Ok((start, finish))
                    }
                    Err(_) => Err("Error parsing date/time value: ".to_string() + s),
                }
            } else {
                Err("Error parsing date/time value: ".to_string() + s)
            }
        }
    }
}

pub fn system_time_to_naive_local(sdt: SystemTime) -> Option<NaiveDateTime> {
    let (sec, nsec) = match sdt.duration_since(UNIX_EPOCH) {
        Ok(dur) => (i64::try_from(dur.as_secs()).ok()?, dur.subsec_nanos()),
        Err(e) => {
            let dur = e.duration();
            let secs = i64::try_from(dur.as_secs()).ok()?;
            if dur.subsec_nanos() == 0 {
                (secs.checked_neg()?, 0)
            } else {
                (secs.checked_neg()?.checked_sub(1)?, 1_000_000_000 - dur.subsec_nanos())
            }
        }
    };

    DateTime::<Utc>::from_timestamp(sec, nsec)
        .map(|dt_utc| dt_utc.with_timezone(&Local).naive_local())
}

pub fn to_local_datetime(dt: &zip::DateTime) -> NaiveDateTime {
    let date = NaiveDate::from_ymd_opt(dt.year() as i32, dt.month() as u32, dt.day() as u32)
        .or_else(|| NaiveDate::from_ymd_opt(dt.year() as i32, 1, 1))
        .unwrap_or_default();
    let time = NaiveTime::from_hms_opt(dt.hour() as u32, dt.minute() as u32, dt.second() as u32)
        .unwrap_or_default();
    NaiveDateTime::new(date, time)
}

pub fn format_datetime(dt: &NaiveDateTime) -> String {
    format!("{}", dt.format("%Y-%m-%d %H:%M:%S"))
}

pub fn format_date(date: &NaiveDate) -> String {
    format!("{}", date.format("%Y-%m-%d"))
}

pub fn format_time(time: &NaiveTime) -> String {
    format!("{}", time.format("%H:%M:%S"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration as StdDuration;
    use chrono::{Datelike, Local, NaiveDate};

    #[test]
    fn test_parse_today() {
        let result = parse_datetime("today").unwrap();
        let now = Local::now().date_naive();
        let start = now.and_hms_opt(0, 0, 0).unwrap();
        let finish = now.and_hms_opt(23, 59, 59).unwrap();

        assert_eq!(result.0, start);
        assert_eq!(result.1, finish);
    }

    #[test]
    fn test_parse_yesterday() {
        let result = parse_datetime("yesterday").unwrap();
        let yesterday = Local::now().date_naive() - chrono::Duration::days(1);
        let start = yesterday.and_hms_opt(0, 0, 0).unwrap();
        let finish = yesterday.and_hms_opt(23, 59, 59).unwrap();

        assert_eq!(result.0, start);
        assert_eq!(result.1, finish);
    }

    #[test]
    fn test_parse_two_days_ago() {
        let result = parse_datetime("2 days ago 00:00").unwrap();
        let two_days_ago = Local::now().date_naive() - chrono::Duration::days(2);
        let start = two_days_ago.and_hms_opt(0, 0, 0).unwrap();
        let finish = two_days_ago.and_hms_opt(23, 59, 59).unwrap();

        assert_eq!(result.0, start);
        assert_eq!(result.1, finish);
    }

    #[test]
    fn test_parse_two_days_ago_simplified() {
        let result = parse_datetime("-2").unwrap();
        let two_days_ago = Local::now().date_naive() - chrono::Duration::days(2);
        let start = two_days_ago.and_hms_opt(0, 0, 0).unwrap();
        let finish = two_days_ago.and_hms_opt(23, 59, 59).unwrap();

        assert_eq!(result.0, start);
        assert_eq!(result.1, finish);
    }

    #[test]
    fn test_parse_specific_date() {
        let result = parse_datetime("2023-12-11").unwrap();
        let date = NaiveDate::from_ymd_opt(2023, 12, 11).unwrap();
        let start = date.and_hms_opt(0, 0, 0).unwrap();
        let finish = date.and_hms_opt(23, 59, 59).unwrap();

        assert_eq!(result.0, start);
        assert_eq!(result.1, finish);
    }

    #[test]
    fn test_parse_specific_datetime() {
        let result = parse_datetime("2023-12-11 14:30:45").unwrap();
        let date = NaiveDate::from_ymd_opt(2023, 12, 11).unwrap();
        let start = date.and_hms_opt(14, 30, 45).unwrap();
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
    fn test_parse_out_of_range_time() {
        assert!(parse_datetime("2024-01-01 25:00:00").is_err());
        assert!(parse_datetime("2024-01-01 12:61:00").is_err());
        assert!(parse_datetime("2024-01-01 12:00:99").is_err());
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
        let date = NaiveDate::from_ymd_opt(2023, 12, 11).unwrap();
        let start = date.and_hms_opt(14, 30, 0).unwrap();
        let finish = date.and_hms_opt(14, 30, 59).unwrap();

        assert_eq!(result.0, start);
        assert_eq!(result.1, finish);
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

        let expected = DateTime::<Utc>::from_timestamp(0, 0)
            .unwrap()
            .with_timezone(&Local)
            .naive_local();
        assert_eq!(result.unwrap(), expected);
    }

    #[test]
    fn test_system_time_to_naive_local_now_roundtrip() {
        let now = SystemTime::now();
        let result = system_time_to_naive_local(now);
        assert!(result.is_some(), "SystemTime::now() must convert");

        let now_local = Local::now().naive_local();
        let diff = (now_local - result.unwrap()).num_seconds().abs();
        assert!(diff < 5, "round-trip drift should be tiny, got {}s", diff);
    }

    #[test]
    fn test_system_time_to_naive_local_before_epoch() {
        let one_sec_before = UNIX_EPOCH - StdDuration::from_secs(1);
        let result = system_time_to_naive_local(one_sec_before);
        assert!(result.is_some(), "pre-epoch SystemTime must convert");

        let expected = DateTime::<Utc>::from_timestamp(-1, 0)
            .unwrap()
            .with_timezone(&Local)
            .naive_local();
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
