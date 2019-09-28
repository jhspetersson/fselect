use chrono::Datelike;
use chrono::DateTime;
use chrono::Duration;
use chrono::Local;
use chrono::LocalResult;
use chrono::Timelike;
use chrono::TimeZone;
use chrono_english::{parse_date_string,Dialect};
use regex::Regex;

lazy_static! {
    static ref DATE_REGEX: Regex = Regex::new("(\\d{4})(-|:)(\\d{1,2})(-|:)(\\d{1,2}) ?(\\d{1,2})?:?(\\d{1,2})?:?(\\d{1,2})?").unwrap();
}

pub fn parse_datetime(s: &str) -> Result<(DateTime<Local>, DateTime<Local>), String> {
    if s == "today" {
        let date = Local::now().date();
        let start = date.and_hms(0, 0, 0);
        let finish = date.and_hms(23, 59, 59);

        return Ok((start, finish));
    }

    if s == "yesterday" {
        let date = Local::now().date() - Duration::days(1);
        let start = date.and_hms(0, 0, 0);
        let finish = date.and_hms(23, 59, 59);

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
                },
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
                },
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
                },
                None => {
                    sec_start = 0;
                    sec_finish = 59;
                }
            }

            match Local.ymd_opt(year, month, day) {
                LocalResult::Single(date) => {
                    let start = date.and_hms(hour_start, min_start, sec_start);
                    let finish = date.and_hms(hour_finish, min_finish, sec_finish);

                    Ok((start, finish))
                },
                _ => Err("Error converting date/time to local: ".to_string() + s)
            }
        },
        None => {
            if s.len() >= 5 {
                match parse_date_string(s, Local::now(), Dialect::Uk) {
                    Ok(date_time) => {
                        let finish;
                        if date_time.hour() == 0 && date_time.minute() == 0 && date_time.second() == 0 {
                            finish = Local.ymd(date_time.year(), date_time.month(), date_time.day())
                                .and_hms(23, 59, 59);
                        } else {
                            finish = date_time;
                        }

                        Ok((date_time, finish))
                    },
                    _ => Err("Error parsing date/time value: ".to_string() + s)
                }
            } else {
                Err("Error parsing date/time value: ".to_string() + s)
            }
        }
    }
}

pub fn to_local_datetime(dt: &zip::DateTime) -> DateTime<Local> {
    Local.ymd(dt.year() as i32, dt.month() as u32, dt.day() as u32)
        .and_hms(dt.hour() as u32, dt.minute() as u32, dt.second() as u32)
}

pub fn format_datetime(dt: &DateTime<Local>) -> String {
    format!("{}", dt.format("%Y-%m-%d %H:%M:%S"))
}