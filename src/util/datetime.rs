use std::sync::LazyLock;

use chrono::{Datelike, Duration, Local, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Timelike};
use chrono_english::{parse_date_string, Dialect};
use regex::Regex;

static DATE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("(\\d{4})(-|:)(\\d{1,2})(-|:)(\\d{1,2}) ?(\\d{1,2})?:?(\\d{1,2})?:?(\\d{1,2})?").unwrap()
});

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
                    let start = date
                        .naive_local()
                        .with_hour(hour_start)
                        .unwrap()
                        .with_minute(min_start)
                        .unwrap()
                        .with_second(sec_start)
                        .unwrap();
                    let finish = date
                        .naive_local()
                        .with_hour(hour_finish)
                        .unwrap()
                        .with_minute(min_finish)
                        .unwrap()
                        .with_second(sec_finish)
                        .unwrap();

                    Ok((start, finish))
                }
                _ => Err("Error converting date/time to local: ".to_string() + s),
            }
        }
        None => {
            if s.len() >= 5 {
                match parse_date_string(s, Local::now(), Dialect::Uk) {
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
                let days = s.parse::<i64>().unwrap();
                let date = Local::now().date_naive() + Duration::days(days);
                let start = date.and_hms_opt(0, 0, 0).unwrap();
                let finish = date.and_hms_opt(23, 59, 59).unwrap();

                Ok((start, finish))
            } else {
                Err("Error parsing date/time value: ".to_string() + s)
            }
        }
    }
}

pub fn to_local_datetime(dt: &zip::DateTime) -> NaiveDateTime {
    Local::now()
        .naive_local()
        .with_year(dt.year() as i32)
        .unwrap()
        .with_month(dt.month() as u32)
        .unwrap()
        .with_day(dt.day() as u32)
        .unwrap()
        .with_hour(dt.hour() as u32)
        .unwrap()
        .with_minute(dt.minute() as u32)
        .unwrap()
        .with_second(dt.second() as u32)
        .unwrap()
}

pub fn format_datetime(dt: &NaiveDateTime) -> String {
    format!("{}", dt.format("%Y-%m-%d %H:%M:%S"))
}

pub fn format_date(date: &NaiveDate) -> String {
    format!("{}", date.format("%Y-%m-%d"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, NaiveDate};

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
    fn test_partial_date_parsing() {
        let result = parse_datetime("2023-12-11 14:30").unwrap();
        let date = NaiveDate::from_ymd_opt(2023, 12, 11).unwrap();
        let start = date.and_hms_opt(14, 30, 0).unwrap();
        let finish = date.and_hms_opt(14, 30, 59).unwrap();
        
        assert_eq!(result.0, start);
        assert_eq!(result.1, finish);
    }
}
