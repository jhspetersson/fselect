use jiff::civil::{Date, DateTime, Time};
use jiff::tz::Offset;
use jiff::{SignedDuration, Span, ZonedDateTime};
use scanlex::{Scanner, Token};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    Uk,
    Us,
}

pub fn parse_date_string(s: &str, now: &ZonedDateTime, dialect: Dialect) -> Option<DateTime> {
    let american = dialect == Dialect::Us;
    let spec = DateParser::new(s, american).parse_date_time()?;
    let time = spec.time.unwrap_or_else(TimeSpec::new_empty);
    match spec.date {
        Some(date) => date.resolve(now, time, american),
        None => time.resolve(now.date(), now),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction {
    Next,
    Last,
    Here,
}

impl Direction {
    fn from_name(s: &str) -> Option<Direction> {
        match s {
            "next" => Some(Direction::Next),
            "last" => Some(Direction::Last),
            _ => None,
        }
    }
}

#[derive(Debug)]
enum ByName {
    WeekDay { direct: Direction, weekday: u32 },
    MonthName { direct: Direction, month: u32 },
    DayMonth { direct: Direction, day: u32, month: u32 },
}

fn civil_date(year: i64, month: u32, day: u32) -> Option<Date> {
    Date::new(i16::try_from(year).ok()?, i8::try_from(month).ok()?, i8::try_from(day).ok()?).ok()
}

fn add_days(date: Date, days: i64) -> Option<Date> {
    date.checked_add(Span::new().try_days(days).ok()?).ok()
}

fn next_last_direction<T: PartialOrd>(date: &T, base: &T, direct: Direction) -> Option<i64> {
    if date > base {
        (direct == Direction::Last).then_some(-1)
    } else if date < base && direct == Direction::Next {
        Some(1)
    } else {
        None
    }
}

impl ByName {
    fn from_name(s: &str, direct: Direction) -> Option<ByName> {
        if let Some(weekday) = week_day(s) {
            Some(ByName::WeekDay { direct, weekday })
        } else {
            month_name(s).map(|month| ByName::MonthName { direct, month })
        }
    }

    fn as_month(&self) -> Option<u32> {
        match *self {
            ByName::MonthName { month, .. } => Some(month),
            _ => None,
        }
    }

    fn resolve(self, now: &ZonedDateTime, ts: TimeSpec, american: bool) -> Option<DateTime> {
        let base = now.datetime();
        let this_year = i64::from(base.year());
        let in_year = |month: u32, day: u32, direct: Direction| {
            let date = civil_date(this_year, month, day)?;
            match next_last_direction(&date.to_datetime(Time::midnight()), &base, direct) {
                Some(correct) => civil_date(this_year + correct, month, day),
                None => Some(date),
            }
        };
        match self {
            ByName::WeekDay { direct, weekday } => {
                let (direct, extra_week) = match direct {
                    Direction::Here => (Direction::Next, 0),
                    Direction::Next if !american => (Direction::Next, 7),
                    other => (other, 0),
                };
                let today = base.date();
                let diff_days = i64::from(weekday) - i64::from(today.weekday().to_monday_zero_offset());
                let mut date = add_days(today, diff_days)?;
                if let Some(correct) = next_last_direction(&date, &today, direct) {
                    date = add_days(date, 7 * correct)?;
                }
                if extra_week > 0 {
                    date = add_days(date, extra_week)?;
                }
                if diff_days == 0 {
                    let this_time = ts.time()?;
                    let base_time = base.time();
                    if let Some(correct) = next_last_direction(&this_time, &base_time, direct) {
                        date = add_days(date, 7 * correct)?;
                    }
                }
                ts.resolve(date, now)
            }
            ByName::MonthName { direct, month } => ts.resolve(in_year(month, 1, direct)?, now),
            ByName::DayMonth { direct, day, month } => ts.resolve(in_year(month, day, direct)?, now),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Interval {
    Seconds(i32),
    Days(i32),
    Months(i32),
}

#[derive(Debug)]
enum DateSpec {
    Absolute { year: u32, month: u32, day: u32 },
    Relative { unit: Interval, skip: i32 },
    FromName(ByName),
}

impl DateSpec {
    fn from_day_month(day: u32, month: u32, direct: Direction) -> DateSpec {
        DateSpec::FromName(ByName::DayMonth { direct, day, month })
    }

    fn resolve(self, now: &ZonedDateTime, ts: TimeSpec, american: bool) -> Option<DateTime> {
        match self {
            DateSpec::Absolute { year, month, day } => ts.resolve(civil_date(i64::from(year), month, day)?, now),
            DateSpec::Relative { unit, skip } => match unit {
                Interval::Seconds(secs) => {
                    let elapsed = SignedDuration::from_secs(i64::from(secs) * i64::from(skip));
                    Some(now.checked_add(elapsed).ok()?.datetime())
                }
                Interval::Days(days) => {
                    let span = Span::new().try_days(i64::from(days) * i64::from(skip)).ok()?;
                    let date_time = now.datetime().checked_add(span).ok()?;
                    if ts.empty {
                        Some(date_time)
                    } else {
                        ts.resolve(date_time.date(), now)
                    }
                }
                Interval::Months(months) => {
                    let span = Span::new().try_months(i64::from(months) * i64::from(skip)).ok()?;
                    ts.resolve(now.date().checked_add(span).ok()?, now)
                }
            },
            DateSpec::FromName(by_name) => by_name.resolve(now, ts, american),
        }
    }
}

#[derive(Debug)]
struct TimeSpec {
    hour: u32,
    min: u32,
    sec: u32,
    empty: bool,
    offset: Option<i64>,
    microsec: u32,
    maybe_token: Option<Token>,
}

impl TimeSpec {
    fn new(hour: u32, min: u32, sec: u32, microsec: u32) -> TimeSpec {
        TimeSpec { hour, min, sec, empty: false, offset: None, microsec, maybe_token: None }
    }

    fn new_with_offset(hour: u32, min: u32, sec: u32, offset: i64, microsec: u32) -> TimeSpec {
        TimeSpec { offset: Some(offset), ..TimeSpec::new(hour, min, sec, microsec) }
    }

    fn new_empty() -> TimeSpec {
        TimeSpec { empty: true, ..TimeSpec::new(0, 0, 0, 0) }
    }

    fn with_token(mut self, token: Option<Token>) -> TimeSpec {
        self.maybe_token = token;
        self
    }

    fn time(&self) -> Option<Time> {
        Time::new(
            i8::try_from(self.hour).ok()?,
            i8::try_from(self.min).ok()?,
            i8::try_from(self.sec).ok()?,
            i32::try_from(self.microsec.checked_mul(1000)?).ok()?,
        )
        .ok()
    }

    fn resolve(&self, date: Date, now: &ZonedDateTime) -> Option<DateTime> {
        let date_time = date.to_datetime(self.time()?);
        match self.offset {
            Some(seconds) => {
                let offset = Offset::from_seconds(i32::try_from(seconds).ok()?).ok()?;
                Some(now.time_zone().to_datetime(offset.to_timestamp(date_time).ok()?))
            }
            None => Some(date_time),
        }
    }
}

struct DateTimeSpec {
    date: Option<DateSpec>,
    time: Option<TimeSpec>,
}

fn week_day(s: &str) -> Option<u32> {
    Some(match s.get(..3)? {
        "sun" => 6,
        "mon" => 0,
        "tue" => 1,
        "wed" => 2,
        "thu" => 3,
        "fri" => 4,
        "sat" => 5,
        _ => return None,
    })
}

fn month_name(s: &str) -> Option<u32> {
    Some(match s.get(..3)? {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    })
}

fn time_unit(s: &str) -> Option<Interval> {
    let name = if s.len() < 3 {
        match s.get(..1)? {
            "s" => "sec",
            "m" => "min",
            "h" => "hou",
            "w" => "wee",
            "d" => "day",
            "y" => "yea",
            _ => return None,
        }
    } else {
        s.get(..3)?
    };
    Some(match name {
        "sec" => Interval::Seconds(1),
        "min" => Interval::Seconds(60),
        "hou" => Interval::Seconds(60 * 60),
        "day" => Interval::Days(1),
        "wee" => Interval::Days(7),
        "mon" => Interval::Months(1),
        "yea" => Interval::Months(12),
        _ => return None,
    })
}

#[derive(Clone, Copy)]
enum TimeKind {
    Formal,
    Informal,
    AmPm(bool),
}

struct DateParser<'a> {
    scanner: Scanner<'a>,
    direct: Direction,
    american: bool,
}

impl<'a> DateParser<'a> {
    fn new(text: &'a str, american: bool) -> DateParser<'a> {
        DateParser { scanner: Scanner::new(text).no_float(), direct: Direction::Here, american }
    }

    fn int(&mut self) -> Option<u32> {
        self.scanner.get_int::<u32>().ok()
    }

    fn iso_date(&mut self, year: u32) -> Option<DateSpec> {
        let month = self.int()?;
        self.scanner.get_ch_matching(&['-']).ok()?;
        let day = self.int()?;
        Some(DateSpec::Absolute { year, month, day })
    }

    fn informal_date(&mut self, day_or_month: u32) -> Option<DateSpec> {
        let month_or_day = self.int()?;
        let (day, month) = if self.american {
            (month_or_day, day_or_month)
        } else {
            (day_or_month, month_or_day)
        };
        Some(if self.scanner.peek() == '/' {
            self.scanner.get();
            let y = self.int()?;
            let year = if y < 100 {
                if y > 40 { 1900 + y } else { 2000 + y }
            } else {
                y
            };
            DateSpec::Absolute { year, month, day }
        } else {
            DateSpec::from_day_month(day, month, self.direct)
        })
    }

    fn parse_date_time(&mut self) -> Option<DateTimeSpec> {
        let mut tspec: Option<TimeSpec> = None;
        let mut t = self.scanner.next()?;
        if let Token::Int(n) = t {
            let ahead = self.scanner.peek();
            if ['.', ':', 'a', 'p'].contains(&ahead) {
                let mut spec = self.parse_time(Some(n as u32))??;
                t = match spec.maybe_token.take() {
                    Some(extra_token) => extra_token,
                    None => self.scanner.next()?,
                };
                if t.finished() {
                    return Some(DateTimeSpec { date: None, time: Some(spec) });
                }
                tspec = Some(spec);
            }
        }
        let sign = t.as_char() == Some('-');
        if sign {
            t = self.scanner.next()?;
        }
        if let Some(name) = t.as_iden() {
            let shortcut = match name {
                "now" | "today" => Some(0),
                "yesterday" => Some(-1),
                "tomorrow" => Some(1),
                _ => None,
            };
            if let Some(skip) = shortcut {
                return Some(DateTimeSpec {
                    date: Some(DateSpec::Relative { unit: Interval::Days(1), skip }),
                    time: tspec,
                });
            } else if let Some(direct) = Direction::from_name(name) {
                self.direct = direct;
            }
        }
        if self.direct != Direction::Here {
            t = self.scanner.next()?;
        }
        let (date, time) = match t {
            Token::Iden(ref name) => {
                let name = name.to_lowercase();
                let by_name = ByName::from_name(&name, self.direct)?;
                let date = if let Some(month) = by_name.as_month() {
                    let t = self.scanner.get();
                    if t.is_integer() {
                        let day = t.to_int_result::<u32>().ok()?;
                        if self.scanner.peek() == ',' {
                            self.scanner.get_char().ok()?;
                            let year = self.int()?;
                            DateSpec::Absolute { year, month, day }
                        } else {
                            DateSpec::from_day_month(day, month, self.direct)
                        }
                    } else if t == Token::End {
                        DateSpec::from_day_month(1, month, self.direct)
                    } else {
                        return None;
                    }
                } else {
                    DateSpec::FromName(by_name)
                };
                if tspec.is_none() {
                    tspec = self.parse_time(None)?;
                }
                (Some(date), tspec)
            }
            Token::Int(_) => {
                let n = t.to_int_result::<u32>().ok()?;
                let t = self.scanner.get();
                if t.finished() {
                    return Some(DateTimeSpec {
                        date: Some(DateSpec::Absolute { year: n, month: 1, day: 1 }),
                        time: None,
                    });
                }
                match t {
                    Token::Iden(ref name) => {
                        let day = n;
                        let name = name.to_lowercase();
                        if let Some(month) = month_name(&name) {
                            let mut hour = None;
                            let date = if let Some(year) = self.int() {
                                let ahead = self.scanner.peek();
                                if ['.', ':', 'a', 'p'].contains(&ahead) {
                                    hour = Some(year);
                                    DateSpec::from_day_month(day, month, self.direct)
                                } else {
                                    DateSpec::Absolute { year, month, day }
                                }
                            } else {
                                DateSpec::from_day_month(day, month, self.direct)
                            };
                            if tspec.is_none() {
                                tspec = self.parse_time(hour)?;
                            }
                            (Some(date), tspec)
                        } else if let Some(unit) = time_unit(&name) {
                            let mut skip = i32::try_from(n).ok()?;
                            if sign {
                                skip = -skip;
                            } else {
                                let t = self.scanner.get();
                                match t.as_iden() {
                                    Some("ago") => skip = -skip,
                                    Some(_) => return None,
                                    None => {
                                        if let Some(hour) = t.to_integer() {
                                            tspec = self.parse_time(Some(hour as u32))?;
                                        }
                                    }
                                }
                            }
                            (Some(DateSpec::Relative { unit, skip }), tspec)
                        } else if name == "am" || name == "pm" {
                            (None, self.parse_time(Some(n))?)
                        } else {
                            return None;
                        }
                    }
                    Token::Char(ch) => {
                        let mut hour = None;
                        let date = match ch {
                            '-' => Some(self.iso_date(n)?),
                            '/' => Some(self.informal_date(n)?),
                            ':' | '.' => {
                                hour = Some(n);
                                None
                            }
                            _ => return None,
                        };
                        if tspec.is_none() {
                            tspec = self.parse_time(hour)?;
                        }
                        (date, tspec)
                    }
                    _ => return None,
                }
            }
            _ => return None,
        };
        Some(DateTimeSpec { date, time })
    }

    fn formal_time(&mut self, hour: u32) -> Option<TimeSpec> {
        let min = self.int()?;
        let mut tnext = None;
        let sec = match self.scanner.next() {
            Some(t) => match t.as_char() {
                Some(':') => self.int()?,
                Some('+' | '-') | None => {
                    tnext = Some(t);
                    0
                }
                Some(_) => return None,
            },
            None => 0,
        };
        if tnext.is_none() {
            tnext = self.scanner.next();
        }
        let micros = if tnext.as_ref().and_then(Token::as_char) == Some('.') {
            let frac = self.scanner.grab_while(char::is_numeric);
            if frac.is_empty() {
                return None;
            }
            let micros = format!("0.{frac}").parse::<f64>().ok()? * 1.0e6;
            tnext = self.scanner.next();
            micros as u32
        } else {
            0
        };
        let Some(tok) = tnext else {
            return Some(TimeSpec::new(hour, min, sec, micros));
        };
        if let Some(ch) = tok.as_char() {
            if ch != '+' && ch != '-' {
                return None;
            }
            self.scanner.skip_whitespace();
            let digits = self.scanner.grab_while(|c| c.is_ascii_digit());
            let hour_and_minute = digits.parse::<u32>().ok()?;
            let (offset_hour, offset_minute) = if self.scanner.peek() == ':' {
                self.scanner.nextch();
                (hour_and_minute, self.int()?)
            } else if digits.len() <= 2 {
                (hour_and_minute, 0)
            } else if digits.len() <= 4 {
                (hour_and_minute / 100, hour_and_minute % 100)
            } else {
                return None;
            };
            let offset = 60 * (i64::from(offset_minute) + 60 * i64::from(offset_hour));
            let offset = if ch == '-' { -offset } else { offset };
            Some(TimeSpec::new_with_offset(hour, min, sec, offset, micros))
        } else if let Some(id) = tok.as_iden() {
            if id == "Z" {
                Some(TimeSpec::new_with_offset(hour, min, sec, 0, micros))
            } else if id == "am" || id == "pm" {
                Some(TimeSpec::new(DateParser::am_pm(id, hour)?, min, sec, micros))
            } else {
                Some(TimeSpec::new(hour, min, sec, micros).with_token(Some(tok)))
            }
        } else {
            Some(TimeSpec::new(hour, min, sec, micros).with_token(Some(tok)))
        }
    }

    fn informal_time(&mut self, hour: u32) -> Option<TimeSpec> {
        let min = self.int()?;
        let tok = self.scanner.get();
        let (hour, next) = match tok.as_iden() {
            Some(name @ ("am" | "pm")) => (DateParser::am_pm(name, hour)?, None),
            _ => (hour, Some(tok)),
        };
        Some(TimeSpec::new(hour, min, 0, 0).with_token(next))
    }

    fn am_pm(name: &str, hour: u32) -> Option<u32> {
        match name {
            "am" if hour == 12 => Some(0),
            "am" => Some(hour),
            "pm" if hour == 12 => Some(12),
            "pm" => hour.checked_add(12),
            _ => None,
        }
    }

    fn hour_time(name: &str, hour: u32) -> Option<TimeSpec> {
        Some(TimeSpec::new(DateParser::am_pm(name, hour)?, 0, 0, 0))
    }

    fn parse_time(&mut self, maybe_hour: Option<u32>) -> Option<Option<TimeSpec>> {
        if let Some(hour) = maybe_hour {
            let kind = match self.scanner.peek() {
                ':' => TimeKind::Formal,
                '.' => TimeKind::Informal,
                'a' => TimeKind::AmPm(false),
                'p' => TimeKind::AmPm(true),
                _ => return None,
            };
            self.scanner.next();
            Some(Some(match kind {
                TimeKind::Formal => self.formal_time(hour)?,
                TimeKind::Informal => self.informal_time(hour)?,
                TimeKind::AmPm(is_pm) => DateParser::hour_time(if is_pm { "pm" } else { "am" }, hour)?,
            }))
        } else {
            if self.scanner.peek() == 'T' {
                self.scanner.nextch();
            }
            let t = self.scanner.get();
            if t.finished() {
                return Some(None);
            }
            let hour = t.to_int_result::<u32>().ok()?;
            Some(Some(match self.scanner.get() {
                Token::Char(':') => self.formal_time(hour)?,
                Token::Char('.') => self.informal_time(hour)?,
                Token::Iden(name) => DateParser::hour_time(&name, hour)?,
                _ => return None,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;
    use jiff::tz::TimeZone;

    fn at(year: i16, month: i8, day: i8, hour: i8, minute: i8) -> ZonedDateTime {
        date(year, month, day).at(hour, minute, 0, 0).to_zoned_datetime(TimeZone::UTC).unwrap()
    }

    fn parse(s: &str, now: &ZonedDateTime, dialect: Dialect) -> String {
        match parse_date_string(s, now, dialect) {
            Some(date_time) => date_time.strftime("%Y-%m-%d %H:%M:%S").to_string(),
            None => String::from("error"),
        }
    }

    #[test]
    fn week_days_are_relative_to_the_base_day() {
        let base = at(2018, 3, 21, 11, 0);
        assert_eq!(parse("friday", &base, Dialect::Uk), "2018-03-23 00:00:00");
        assert_eq!(parse("friday 10:30", &base, Dialect::Uk), "2018-03-23 10:30:00");
        assert_eq!(parse("friday 8pm", &base, Dialect::Uk), "2018-03-23 20:00:00");
        assert_eq!(parse("tues", &base, Dialect::Uk), "2018-03-27 00:00:00");
        assert_eq!(parse("next mon", &base, Dialect::Us), "2018-03-26 00:00:00");
        assert_eq!(parse("next mon", &base, Dialect::Uk), "2018-04-02 00:00:00");
        assert_eq!(parse("last fri 9.30", &base, Dialect::Uk), "2018-03-16 09:30:00");
        assert_eq!(parse("wednesday", &base, Dialect::Uk), "2018-03-28 00:00:00");
        assert_eq!(parse("wednesday 12:00", &base, Dialect::Uk), "2018-03-21 12:00:00");
        assert_eq!(parse("last wednesday 12:00", &base, Dialect::Uk), "2018-03-14 12:00:00");
    }

    #[test]
    fn day_and_month_take_the_base_year_unless_directed() {
        let base = at(2018, 3, 21, 11, 0);
        assert_eq!(parse("9/11", &base, Dialect::Us), "2018-09-11 00:00:00");
        assert_eq!(parse("9/11", &base, Dialect::Uk), "2018-11-09 00:00:00");
        assert_eq!(parse("last 9/11", &base, Dialect::Us), "2017-09-11 00:00:00");
        assert_eq!(parse("last 9/11 9am", &base, Dialect::Us), "2017-09-11 09:00:00");
        assert_eq!(parse("April 1 8.30pm", &base, Dialect::Uk), "2018-04-01 20:30:00");
        assert_eq!(parse("1 April 8.30pm", &base, Dialect::Uk), "2018-04-01 20:30:00");
        assert_eq!(parse("May", &base, Dialect::Uk), "2018-05-01 00:00:00");
        assert_eq!(parse("next jan", &base, Dialect::Uk), "2019-01-01 00:00:00");
        assert_eq!(parse("last May", &base, Dialect::Uk), "2017-05-01 00:00:00");
        assert_eq!(parse("29 feb", &base, Dialect::Uk), "error");
    }

    #[test]
    fn intervals_move_from_the_base_time() {
        let base = at(2018, 3, 21, 11, 0);
        assert_eq!(parse("2d", &base, Dialect::Uk), "2018-03-23 11:00:00");
        assert_eq!(parse("2d 03:00", &base, Dialect::Uk), "2018-03-23 03:00:00");
        assert_eq!(parse("3 weeks", &base, Dialect::Uk), "2018-04-11 11:00:00");
        assert_eq!(parse("3h", &base, Dialect::Uk), "2018-03-21 14:00:00");
        assert_eq!(parse("3 hours ago", &base, Dialect::Uk), "2018-03-21 08:00:00");
        assert_eq!(parse(" -3h", &base, Dialect::Uk), "2018-03-21 08:00:00");
        assert_eq!(parse("6 months", &base, Dialect::Uk), "2018-09-21 00:00:00");
        assert_eq!(parse("6 months ago", &base, Dialect::Uk), "2017-09-21 00:00:00");
        assert_eq!(parse(" -3 month", &base, Dialect::Uk), "2017-12-21 00:00:00");
        assert_eq!(parse("yesterday", &base, Dialect::Uk), "2018-03-20 11:00:00");
        assert_eq!(parse("15:30 yesterday", &base, Dialect::Uk), "2018-03-20 15:30:00");
        assert_eq!(parse("8pm tomorrow", &base, Dialect::Uk), "2018-03-22 20:00:00");
        assert_eq!(parse("8.30 today", &base, Dialect::Uk), "2018-03-21 08:30:00");
        assert_eq!(parse("2 days later", &base, Dialect::Uk), "error");
    }

    #[test]
    fn month_intervals_clamp_the_day_and_cross_years() {
        let end_of_january = at(2024, 1, 31, 23, 59);
        assert_eq!(parse("1 month", &end_of_january, Dialect::Uk), "2024-02-29 00:00:00");
        assert_eq!(parse("1 month ago", &end_of_january, Dialect::Uk), "2023-12-31 00:00:00");
        assert_eq!(parse("1 year ago", &end_of_january, Dialect::Uk), "2023-01-31 00:00:00");
        assert_eq!(parse("12 months ago", &end_of_january, Dialect::Uk), "2023-01-31 00:00:00");
        assert_eq!(parse("13 months", &end_of_january, Dialect::Uk), "2025-02-28 00:00:00");
        assert_eq!(parse("6 months ago", &at(2021, 7, 4, 17, 45), Dialect::Uk), "2021-01-04 00:00:00");
    }

    #[test]
    fn absolute_dates_accept_formal_and_informal_forms() {
        let base = at(2018, 3, 21, 11, 0);
        assert_eq!(parse("2017-06-30", &base, Dialect::Uk), "2017-06-30 00:00:00");
        assert_eq!(parse("30/06/17", &base, Dialect::Uk), "2017-06-30 00:00:00");
        assert_eq!(parse("06/30/17", &base, Dialect::Us), "2017-06-30 00:00:00");
        assert_eq!(parse("30/06/41", &base, Dialect::Uk), "1941-06-30 00:00:00");
        assert_eq!(parse("30 June 2018", &base, Dialect::Uk), "2018-06-30 00:00:00");
        assert_eq!(parse("June 30, 2018", &base, Dialect::Uk), "2018-06-30 00:00:00");
        assert_eq!(parse("June   30,    2018", &base, Dialect::Uk), "2018-06-30 00:00:00");
        assert_eq!(parse("2018", &base, Dialect::Uk), "2018-01-01 00:00:00");
        assert_eq!(parse("31 June 2018", &base, Dialect::Uk), "error");
        assert_eq!(parse("2017-02-29", &base, Dialect::Uk), "error");
        assert_eq!(parse("12345", &base, Dialect::Uk), "error");
    }

    #[test]
    fn times_follow_or_precede_the_date() {
        let base = at(2018, 3, 21, 11, 0);
        assert_eq!(parse("2017-06-30 08:20:30", &base, Dialect::Uk), "2017-06-30 08:20:30");
        assert_eq!(parse("2017-06-30T08:20:30", &base, Dialect::Uk), "2017-06-30 08:20:30");
        assert_eq!(parse("2017-06-30 8.20", &base, Dialect::Uk), "2017-06-30 08:20:00");
        assert_eq!(parse("2017-06-30 8.30pm", &base, Dialect::Uk), "2017-06-30 20:30:00");
        assert_eq!(parse("2017-06-30 8:30pm", &base, Dialect::Uk), "2017-06-30 20:30:00");
        assert_eq!(parse("2017-06-30 2am", &base, Dialect::Uk), "2017-06-30 02:00:00");
        assert_eq!(parse("08:20:30 2017-06-30", &base, Dialect::Uk), "2017-06-30 08:20:30");
        assert_eq!(parse("8.30pm 2017-06-30", &base, Dialect::Uk), "2017-06-30 20:30:00");
        assert_eq!(parse("2am 2017-06-30", &base, Dialect::Uk), "2017-06-30 02:00:00");
        assert_eq!(parse("8.30", &base, Dialect::Uk), "2018-03-21 08:30:00");
        assert_eq!(parse("2017-06-30 25:00", &base, Dialect::Uk), "error");
        assert_eq!(parse("2017-06-30 13pm", &base, Dialect::Uk), "error");
    }

    #[test]
    fn utc_offsets_convert_to_the_base_time_zone() {
        let base = at(2018, 3, 21, 11, 0);
        assert_eq!(parse("2017-06-30 08:20:30 +02:00", &base, Dialect::Uk), "2017-06-30 06:20:30");
        assert_eq!(parse("2017-06-30 08:20:30 +0200", &base, Dialect::Uk), "2017-06-30 06:20:30");
        assert_eq!(parse("2017-06-30T08:20:30Z", &base, Dialect::Uk), "2017-06-30 08:20:30");
        assert_eq!(parse("08:20:30 +02:00 2017-06-30", &base, Dialect::Uk), "2017-06-30 06:20:30");
        assert_eq!(parse("2017-06-30T08:20:30+02", &base, Dialect::Uk), "2017-06-30 06:20:30");
        assert_eq!(parse("2017-06-30T08:20:30-5", &base, Dialect::Uk), "2017-06-30 13:20:30");
        assert_eq!(parse("2017-06-30T08:20:30+0030", &base, Dialect::Uk), "2017-06-30 07:50:30");
        assert_eq!(parse("2017-06-30T08:20:30+530", &base, Dialect::Uk), "2017-06-30 02:50:30");
        assert_eq!(parse("2017-06-30T08:20+02:00", &base, Dialect::Uk), "2017-06-30 06:20:00");
        assert_eq!(parse("2017-06-30 08:20 -0530", &base, Dialect::Uk), "2017-06-30 13:50:00");
        assert_eq!(parse("2017-06-30T08:20Z", &base, Dialect::Uk), "2017-06-30 08:20:00");
        assert_eq!(parse("2017-06-30T08:20:30+", &base, Dialect::Uk), "error");
        assert_eq!(parse("2017-06-30T08:20:30+02000", &base, Dialect::Uk), "error");

        let tokyo = date(2018, 3, 21).at(11, 0, 0, 0).to_zoned_datetime(TimeZone::fixed(jiff::tz::offset(9))).unwrap();
        assert_eq!(parse("2017-06-30T08:20:30Z", &tokyo, Dialect::Uk), "2017-06-30 17:20:30");
        assert_eq!(parse("2017-06-30 23:20:30 -01:00", &tokyo, Dialect::Uk), "2017-07-01 09:20:30");
    }

    #[test]
    fn malformed_input_is_rejected_without_panicking() {
        let base = at(2018, 3, 21, 11, 0);
        for s in [
            "",
            "   ",
            "bananas",
            "next",
            "1 \u{e9}\u{e9}",
            "\u{e9}\u{e9}\u{e9} 1",
            "2024\u{e9}",
            "4294967295pm",
            "99999999999 years",
            "1 June 2024 10:30 +99:00",
        ] {
            assert_eq!(parse(s, &base, Dialect::Uk), "error", "{:?}", s);
        }
    }
}
