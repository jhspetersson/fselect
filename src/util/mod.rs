mod top_n;
mod wbuf;

use std::cmp::Ordering;
use std::error::Error;
use std::fmt::Display;
use std::fs::{DirEntry, File};
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::string::ToString;

use chrono::DateTime;
use chrono::Duration;
use chrono::Local;
use chrono::LocalResult;
use chrono::TimeZone;
use chrono_english::{parse_date_string,Dialect};
use regex::Regex;
use sha1::Digest;
use term;
use term::StdoutTerminal;
use time::Tm;

pub use self::top_n::TopN;
pub use self::wbuf::WritableBuffer;
use crate::parser::ColumnExpr;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd)]
pub struct Criteria<T> where T: Display + ToString {
    fields: Rc<Vec<ColumnExpr>>,
    /// Values of current row to sort with, placed in order of significance.
    values: Vec<T>,
    /// Shared smart reference to Vector of boolean where each index corresponds to whether the
    /// field at that index should be ordered in ascending order `true` or descending order `false`.
    orderings: Rc<Vec<bool>>,
}

impl<T> Criteria<T> where T: Display {
    pub fn new(fields: Rc<Vec<ColumnExpr>>, values: Vec<T>, orderings: Rc<Vec<bool>>) -> Criteria<T> {
        debug_assert_eq!(fields.len(), values.len());
        debug_assert_eq!(values.len(), orderings.len());

        Criteria { fields, values, orderings }
    }

    #[inline]
    fn cmp_at(&self, other: &Self, i: usize) -> Ordering where T: Ord {
        let field = &self.fields[i];
        let comparison = match &field.field {
            Some(field) => {
                if field.is_numeric_field() {
                    self.cmp_at_numbers(other, i)
                } else if field.is_datetime_field() {
                    self.cmp_at_datetimes(other, i)
                } else {
                    self.cmp_at_direct(other, i)
                }
            },
            _ => self.cmp_at_direct(other, i)
        };

        if self.orderings[i] { comparison } else { comparison.reverse() }
    }

    #[inline]
    fn cmp_at_direct(&self, other: &Self, i: usize) -> Ordering where T: Ord {
        if self.values[i] < other.values[i] {
            Ordering::Less
        } else if self.values[i] > other.values[i] {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    }

    #[inline]
    fn cmp_at_numbers(&self, other: &Self, i: usize) -> Ordering where T: Ord {
        let a = parse_filesize(&self.values[i].to_string()).unwrap_or(0);
        let b = parse_filesize(&other.values[i].to_string()).unwrap_or(0);

        if a < b {
            Ordering::Less
        } else if a > b {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    }

    #[inline]
    fn cmp_at_datetimes(&self, other: &Self, i: usize) -> Ordering where T: Ord {
        let default = Local.ymd(1970, 1, 1).and_hms(0, 0, 0);
        let a = parse_datetime(&self.values[i].to_string()).unwrap_or((default, default)).0;
        let b = parse_datetime(&other.values[i].to_string()).unwrap_or((default, default)).0;

        if a < b {
            Ordering::Less
        } else if a > b {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    }
}

impl<T: Display + Ord> Ord for Criteria<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        for i in 0..(self.values.len().min(other.values.len())) {
            let ord = self.cmp_at(other, i);
            if ord != Ordering::Equal {
                return ord;
            }
        }

        self.values.len().cmp(&other.values.len())
    }
}

pub fn path_error_message(p: &Path, e: io::Error, t: &mut Box<StdoutTerminal>) {
    error_message(&p.to_string_lossy(), e.description(), t);
}

pub fn error_message(source: &str, description: &str, t: &mut Box<StdoutTerminal>) {
    t.fg(term::color::YELLOW).unwrap();
    eprint!("{}", source);
    t.reset().unwrap();

    eprint!(": ");

    t.fg(term::color::RED).unwrap();
    eprintln!("{}", description);
    t.reset().unwrap();
}

pub fn parse_filesize(s: &str) -> Option<u64> {
    let string = s.to_string().to_ascii_lowercase();

    if string.ends_with("k") {
        match &string[..(s.len() - 1)].parse::<u64>() {
            Ok(size) => return Some(size * 1024),
            _ => return None
        }
    }

    if string.ends_with("kb") {
        match &string[..(s.len() - 2)].parse::<u64>() {
            Ok(size) => return Some(size * 1024),
            _ => return None
        }
    }

    if string.ends_with("kib") {
        match &string[..(s.len() - 3)].parse::<u64>() {
            Ok(size) => return Some(size * 1024),
            _ => return None
        }
    }

    if string.ends_with("m") {
        match &string[..(s.len() - 1)].parse::<u64>() {
            Ok(size) => return Some(size * 1024 * 1024),
            _ => return None
        }
    }

    if string.ends_with("mb") {
        match &string[..(s.len() - 2)].parse::<u64>() {
            Ok(size) => return Some(size * 1024 * 1024),
            _ => return None
        }
    }

    if string.ends_with("mib") {
        match &string[..(s.len() - 3)].parse::<u64>() {
            Ok(size) => return Some(size * 1024 * 1024),
            _ => return None
        }
    }

    if string.ends_with("g") {
        match &string[..(s.len() - 1)].parse::<u64>() {
            Ok(size) => return Some(size * 1024 * 1024 * 1024),
            _ => return None
        }
    }

    if string.ends_with("gb") {
        match &string[..(s.len() - 2)].parse::<u64>() {
            Ok(size) => return Some(size * 1024 * 1024 * 1024),
            _ => return None
        }
    }

    if string.ends_with("gib") {
        match &string[..(s.len() - 3)].parse::<u64>() {
            Ok(size) => return Some(size * 1024 * 1024 * 1024),
            _ => return None
        }
    }

    match string.parse::<u64>() {
        Ok(size) => return Some(size),
        _ => return None
    }
}

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
                    min_finish = 23;
                }
            }

            let sec_start: u32;
            let sec_finish: u32;
            match cap.get(8) {
                Some(val) => {
                    sec_start = val.as_str().parse().unwrap();
                    sec_finish = min_start;
                },
                None => {
                    sec_start = 0;
                    sec_finish = 23;
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
            match parse_date_string(s, Local::now(), Dialect::Uk) {
                Ok(date_time) => Ok((date_time, date_time)),
                _ => Err("Error parsing date/time value: ".to_string() + s)
            }
        }
    }
}

pub fn to_local_datetime(tm: &Tm) -> DateTime<Local> {
    Local.ymd(tm.tm_year + 1900, (tm.tm_mon + 1) as u32, tm.tm_mday as u32)
        .and_hms(tm.tm_hour as u32, tm.tm_min as u32, tm.tm_sec as u32)
}

pub fn str_to_bool(val: &str) -> bool {
    let str_val = val.to_ascii_lowercase();
    str_val.eq("true") || str_val.eq("1")
}

pub fn parse_unix_filename(s: &str) -> &str {
    let last_slash = s.rfind('/');
    match last_slash {
        Some(idx) => &s[idx..],
        _ => s
    }
}

pub fn format_absolute_path(path_buf: &PathBuf) -> String {
    let path = format!("{}", path_buf.to_string_lossy());

    #[cfg(windows)]
    let path = path.replace("\\\\?\\", "");

    path
}

pub fn get_sha1_file_hash(entry: &DirEntry) -> String {
    if let Ok(mut file) = File::open(&entry.path()) {
        let mut hasher = sha1::Sha1::new();
        if io::copy(&mut file, &mut hasher).is_ok() {
            let hash = hasher.result();
            return format!("{:x}", hash);
        }
    }

    String::new()
}

pub fn get_sha256_file_hash(entry: &DirEntry) -> String {
    if let Ok(mut file) = File::open(&entry.path()) {
        let mut hasher = sha2::Sha256::new();
        if io::copy(&mut file, &mut hasher).is_ok() {
            let hash = hasher.result();
            return format!("{:x}", hash);
        }
    }

    String::new()
}

pub fn get_sha512_file_hash(entry: &DirEntry) -> String {
    if let Ok(mut file) = File::open(&entry.path()) {
        let mut hasher = sha2::Sha512::new();
        if io::copy(&mut file, &mut hasher).is_ok() {
            let hash = hasher.result();
            return format!("{:x}", hash);
        }
    }

    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::Field;

    fn basic_criteria<T: Ord + Clone + Display>(vals: &[T]) -> Criteria<T> {
        let fields = Rc::new(vec![ColumnExpr::field(Field::Size); vals.len()]);
        let orderings = Rc::new(vec![true; vals.len()]);

        Criteria::new(fields, vals.to_vec(), orderings)
    }

    #[test]
    fn test_compare_same() {
        let c1 = basic_criteria(&[1, 2, 3]);
        let c2 = basic_criteria(&[1, 2, 3]);

        assert_eq!(c1.cmp(&c2), Ordering::Equal);
    }

    #[test]
    fn test_compare_first_smaller() {
        let c1 = basic_criteria(&[1, 2, 3]);
        let c2 = basic_criteria(&[3, 2, 3]);

        assert_eq!(c1.cmp(&c2), Ordering::Less);
    }

    #[test]
    fn test_compare_first_smaller_same_prefix() {
        let c1 = basic_criteria(&[1, 2, 3]);
        let c2 = basic_criteria(&[1, 3, 3]);

        assert_eq!(c1.cmp(&c2), Ordering::Less);
    }

    #[test]
    fn test_compare_shorter_smaller_same_prefix() {
        let c1 = basic_criteria(&[1, 2, 3]);
        let c2 = basic_criteria(&[1, 2, 3, 4]);

        assert_eq!(c1.cmp(&c2), Ordering::Less);
    }

    #[test]
    fn test_compare_all_fields_reverse() {
        let fields = Rc::new(vec![ColumnExpr::field(Field::Size); 3]);
        let orderings = Rc::new(vec![false, false, false]);

        let c1 = Criteria::new(fields.clone(), vec![1, 2, 3], orderings.clone());
        let c2 = Criteria::new(fields.clone(), vec![1, 3, 1], orderings.clone());

        assert_eq!(c1.cmp(&c2), Ordering::Greater);
    }

    #[test]
    fn test_compare_some_fields_reverse() {
        let fields = Rc::new(vec![ColumnExpr::field(Field::Size); 3]);
        let orderings = Rc::new(vec![true, false, true]);

        let c1 = Criteria::new(fields.clone(), vec![1, 2, 3], orderings.clone());
        let c2 = Criteria::new(fields.clone(), vec![1, 3, 1], orderings.clone());

        assert_eq!(c1.cmp(&c2), Ordering::Greater);
    }
}
