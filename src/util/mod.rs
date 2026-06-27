#[cfg(target_os = "linux")]
pub(crate) mod acl;
pub(crate) mod app_dirs;
pub mod audio;
#[cfg(target_os = "linux")]
pub(crate) mod capabilities;
#[cfg(target_os = "linux")]
pub(crate) mod extattrs;
#[cfg(windows)]
pub(crate) mod win_acl;
#[cfg(windows)]
pub(crate) mod win_attrs;
#[cfg(windows)]
pub(crate) mod win_xattr;
pub(crate) mod datetime;
#[cfg(all(windows, feature = "everything"))]
pub(crate) mod everything;
#[cfg(all(unix, feature = "plocate"))]
pub(crate) mod plocate;
pub mod dimensions;
pub mod duration;
pub(crate) mod error;
#[cfg(feature = "git")]
pub(crate) mod git;
mod glob;
pub(crate) mod greek;
pub(crate) mod japanese;
mod top_n;
pub(crate) mod variant;
mod wbuf;

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt::Display;
use std::fs;
use std::fs::canonicalize;
use std::fs::symlink_metadata;
use std::fs::DirEntry;
use std::fs::File;
use std::fs::Metadata;
use std::io::Read;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::LazyLock;

use chrono::{NaiveDate, NaiveDateTime};
use regex::Regex;

pub use self::datetime::format_date;
pub use self::datetime::format_datetime;
pub use self::datetime::format_time;
pub use self::datetime::parse_datetime;
pub use self::datetime::set_us_dates;
pub use self::datetime::system_time_to_naive_local;
pub use self::datetime::to_local_datetime;
pub use self::glob::convert_glob_to_pattern;
pub use self::glob::convert_like_to_pattern;
pub use self::glob::is_glob;
pub use self::top_n::TopN;
pub use self::variant::{Variant, VariantType};
pub use self::wbuf::WritableBuffer;
use crate::expr::Expr;
#[cfg(windows)]
use crate::mode;
pub use audio::AudioInfo;
pub use dimensions::Dimensions;
pub use duration::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Criteria<T>
where
    T: Display + ToString,
{
    fields: Rc<Vec<Expr>>,
    /// Values of current row to sort with, placed in order of significance.
    values: Vec<T>,
    /// Shared smart reference to Vector of boolean where each index corresponds to whether the
    /// field at that index should be ordered in ascending order `true` or descending order `false`.
    orderings: Rc<Vec<bool>>,
}

impl<T> Criteria<T>
where
    T: Display,
{
    pub fn new(fields: Rc<Vec<Expr>>, values: Vec<T>, orderings: Rc<Vec<bool>>) -> Criteria<T> {
        debug_assert_eq!(fields.len(), values.len());
        debug_assert_eq!(values.len(), orderings.len());

        Criteria {
            fields,
            values,
            orderings,
        }
    }

    #[inline]
    fn cmp_at(&self, other: &Self, i: usize) -> Ordering
    where
        T: Ord,
    {
        let field = &self.fields[i];
        let comparison;
        if field.contains_numeric() {
            comparison = self.cmp_at_numbers(other, i);
        } else if field.contains_datetime() {
            comparison = self.cmp_at_datetimes(other, i);
        } else {
            comparison = self.cmp_at_direct(other, i);
        }

        if self.orderings[i] {
            comparison
        } else {
            comparison.reverse()
        }
    }

    #[inline]
    fn cmp_at_direct(&self, other: &Self, i: usize) -> Ordering
    where
        T: Ord,
    {
        self.values[i].cmp(&other.values[i])
    }

    #[inline]
    fn cmp_at_numbers(&self, other: &Self, i: usize) -> Ordering
    where
        T: Ord,
    {
        let a_str = self.values[i].to_string();
        let b_str = other.values[i].to_string();

        match (numeric_sort_key(&a_str), numeric_sort_key(&b_str)) {
            (Some(a), Some(b)) => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
            // Numbers sort before non-numbers, so missing values land last
            // in ascending order.
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            // Neither side is numeric: fall back to plain string order. This
            // keeps datetime values produced by numeric expressions (e.g.
            // `max(modified)`, formatted as fixed-width YYYY-MM-DD HH:MM:SS)
            // in chronological order instead of collapsing every key to zero.
            (None, None) => a_str.cmp(&b_str),
        }
    }

    #[inline]
    fn cmp_at_datetimes(&self, other: &Self, i: usize) -> Ordering
    where
        T: Ord,
    {
        static EPOCH: LazyLock<NaiveDateTime> = LazyLock::new(|| {
            NaiveDate::from_ymd_opt(1970, 1, 1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
        });
        let default = *EPOCH;
        let a = parse_datetime(&self.values[i].to_string())
            .unwrap_or((default, default))
            .0;
        let b = parse_datetime(&other.values[i].to_string())
            .unwrap_or((default, default))
            .0;

        a.cmp(&b)
    }
}

impl<T: Display + Ord> PartialOrd for Criteria<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
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

/// Numeric sort key for ORDER BY comparisons: a plain integer or float, or a
/// file size with a unit suffix ("1k", "5mb"). `None` for anything else.
fn numeric_sort_key(s: &str) -> Option<f64> {
    if let Ok(num) = s.parse::<f64>()
        && num.is_finite() {
            return Some(num);
        }
    parse_filesize(s).map(|size| size as f64)
}

#[cfg(windows)]
pub fn calc_depth(s: &str) -> u32 {
    s.matches("\\").count() as u32
}

#[cfg(not(windows))]
pub fn calc_depth(s: &str) -> u32 {
    s.matches("/").count() as u32
}

pub fn get_extension(s: &str) -> String {
    match Path::new(s).extension() {
        Some(ext) => ext.to_string_lossy().to_string(),
        None => String::new(),
    }
}

pub fn get_stem(s: &str) -> String {
    match Path::new(s).file_stem() {
        Some(stem) => stem.to_string_lossy().to_string(),
        None => String::new(),
    }
}

pub fn parse_filesize(s: &str) -> Option<u64> {
    let string = s.to_string().to_ascii_lowercase().replace(" ", "");
    let length = string.len();

    if length > 1 && string.ends_with("k") {
        return match &string[..(length - 1)].parse::<f64>() {
            Ok(size) => Some((*size * 1024.0) as u64),
            _ => None,
        };
    }

    if length > 2 && string.ends_with("kb") {
        return match &string[..(length - 2)].parse::<f64>() {
            Ok(size) => Some((*size * 1000.0) as u64),
            _ => None,
        };
    }

    if length > 3 && string.ends_with("kib") {
        return match &string[..(length - 3)].parse::<f64>() {
            Ok(size) => Some((*size * 1024.0) as u64),
            _ => None,
        };
    }

    if length > 1 && string.ends_with("m") {
        return match &string[..(length - 1)].parse::<f64>() {
            Ok(size) => Some((*size * 1024.0 * 1024.0) as u64),
            _ => None,
        };
    }

    if length > 2 && string.ends_with("mb") {
        return match &string[..(length - 2)].parse::<f64>() {
            Ok(size) => Some((*size * 1000.0 * 1000.0) as u64),
            _ => None,
        };
    }

    if length > 3 && string.ends_with("mib") {
        return match &string[..(length - 3)].parse::<f64>() {
            Ok(size) => Some((*size * 1024.0 * 1024.0) as u64),
            _ => None,
        };
    }

    if length > 1 && string.ends_with("g") {
        return match &string[..(length - 1)].parse::<f64>() {
            Ok(size) => Some((*size * 1024.0 * 1024.0 * 1024.0) as u64),
            _ => None,
        };
    }

    if length > 2 && string.ends_with("gb") {
        return match &string[..(length - 2)].parse::<f64>() {
            Ok(size) => Some((*size * 1000.0 * 1000.0 * 1000.0) as u64),
            _ => None,
        };
    }

    if length > 3 && string.ends_with("gib") {
        return match &string[..(length - 3)].parse::<f64>() {
            Ok(size) => Some((*size * 1024.0 * 1024.0 * 1024.0) as u64),
            _ => None,
        };
    }

    if length > 1 && string.ends_with("b") {
        return match &string[..(length - 1)].parse::<u64>() {
            Ok(size) => Some(*size),
            _ => None,
        };
    }

    string.parse::<u64>().ok()
}

static FILE_SIZE_FORMAT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("(%\\.(?P<zeroes>\\d+))?(?P<space>\\s)?(?P<units>\\w+)?").unwrap()
});

pub fn format_filesize(size: u64, modifier: &str) -> Result<String, String> {
    let mut modifier = modifier.to_ascii_lowercase();

    let mut zeroes = -1;
    let mut space = false;

    if let Some(cap) = FILE_SIZE_FORMAT_REGEX.captures(&modifier) {
        zeroes = cap
            .name("zeroes")
            .map_or(-1, |m| m.as_str().parse::<i32>().unwrap_or(20));
        space = cap.name("space").is_some_and(|m| m.as_str() == " ");
        modifier = cap
            .name("units")
            .map_or(String::from(""), |m| m.as_str().to_string());
    }

    let fixed_at;
    let mut format;

    let conventional;
    if modifier.contains("c") {
        conventional = true;
        modifier = modifier.replace("c", "");
    } else {
        conventional = false;
    }

    let decimal;
    if modifier.contains("d") {
        decimal = true;
        modifier = modifier.replace("d", "");
    } else {
        decimal = false;
    }

    let short_units;
    if modifier.contains("s") {
        short_units = true;
        modifier = modifier.replace("s", "");
    } else {
        short_units = false;
    }

    match modifier.as_str() {
        "b" | "byte" => {
            fixed_at = Some(humansize::FixedAt::Base);
            format = humansize::BINARY;
        }
        "k" | "kib" => {
            fixed_at = Some(humansize::FixedAt::Kilo);
            format = humansize::BINARY;
            if zeroes == -1 {
                zeroes = 0;
            }
        }
        "kb" => {
            fixed_at = Some(humansize::FixedAt::Kilo);
            format = humansize::DECIMAL;
            if zeroes == -1 {
                zeroes = 0;
            }
        }
        "m" | "mib" => {
            fixed_at = Some(humansize::FixedAt::Mega);
            format = humansize::BINARY;
            if zeroes == -1 {
                zeroes = 0;
            }
        }
        "mb" => {
            fixed_at = Some(humansize::FixedAt::Mega);
            format = humansize::DECIMAL;
            if zeroes == -1 {
                zeroes = 0;
            }
        }
        "g" | "gib" => {
            fixed_at = Some(humansize::FixedAt::Giga);
            format = humansize::BINARY;
        }
        "gb" => {
            fixed_at = Some(humansize::FixedAt::Giga);
            format = humansize::DECIMAL;
        }
        "t" | "tib" => {
            fixed_at = Some(humansize::FixedAt::Tera);
            format = humansize::BINARY;
        }
        "tb" => {
            fixed_at = Some(humansize::FixedAt::Tera);
            format = humansize::DECIMAL;
        }
        "p" | "pib" => {
            fixed_at = Some(humansize::FixedAt::Peta);
            format = humansize::BINARY;
        }
        "pb" => {
            fixed_at = Some(humansize::FixedAt::Peta);
            format = humansize::DECIMAL;
        }
        "e" | "eib" => {
            fixed_at = Some(humansize::FixedAt::Exa);
            format = humansize::BINARY;
        }
        "eb" => {
            fixed_at = Some(humansize::FixedAt::Exa);
            format = humansize::DECIMAL;
        }
        "" => {
            fixed_at = None;
            format = humansize::BINARY;
        }
        _ => {
            return Err(format!("Invalid file size format modifier: {}", modifier));
        }
    };

    if zeroes == -1 {
        zeroes = 2;
    }

    if conventional {
        format = humansize::WINDOWS;
    }

    if decimal {
        format = humansize::DECIMAL;
    }

    let format_options = humansize::FormatSizeOptions::from(format)
        .fixed_at(fixed_at)
        .decimal_places(zeroes as usize)
        .space_after_value(space);

    let mut result = humansize::format_size(size, format_options).replace("kB", "KB");

    if short_units {
        result = result
            .replace("iB", "")
            .replace("KB", "K")
            .replace("MB", "M")
            .replace("GB", "G")
            .replace("TB", "T")
            .replace("PB", "P")
            .replace("EB", "E");
    }

    Ok(result)
}

pub fn str_to_bool(val: &str) -> Option<bool> {
    let str_val = val.to_ascii_lowercase();
    match str_val.as_str() {
        "true" | "t" | "1" | "yes" | "y" | "on" => Some(true),
        "false" | "f" | "0" | "no" | "n" | "off" => Some(false),
        _ => None,
    }
}

pub fn capitalize_initials(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut prev_boundary = true;
    for c in s.chars() {
        if !c.is_alphanumeric() {
            result.push(c);
            prev_boundary = true;
        } else if prev_boundary {
            for uc in c.to_uppercase() {
                result.push(uc);
            }
            prev_boundary = false;
        } else {
            for lc in c.to_lowercase() {
                result.push(lc);
            }
            prev_boundary = false;
        }
    }
    result
}

pub fn parse_unix_filename(s: &str) -> &str {
    let last_slash = s.rfind('/');
    match last_slash {
        Some(idx) => &s[idx + 1..],
        _ => s,
    }
}

pub fn has_extension(file_name: &str, extensions: &Vec<String>) -> bool {
    let s = file_name.to_ascii_lowercase();

    for ext in extensions {
        if s.ends_with(ext) {
            return true;
        }
    }

    false
}

pub fn looks_like_regexp(s: &str) -> bool {
    s.contains('*') || s.contains('[') || s.contains('?')
}

pub fn is_text_mime(mime: &str) -> bool {
    mime.starts_with("text/")
        || mime.contains("+xml")
        || mime.contains("-xml")
        || mime.eq("application/x-awk")
        || mime.eq("application/x-perl")
        || mime.eq("application/x-php")
        || mime.eq("application/x-ruby")
        || mime.eq("application/x-shellscript")
}

pub fn canonical_path(path_buf: &PathBuf) -> Result<String, String> {
    match canonicalize(path_buf) {
        Ok(path) => Ok(format_absolute_path(&path)),
        Err(err) => {
            // WASI and some other platforms don't support canonicalize;
            // fall back to the path as-is
            if err.raw_os_error() == Some(58) || err.to_string().starts_with("Incorrect function.") {
                Ok(format_absolute_path(path_buf))
            } else {
                Err(err.to_string())
            }
        },
    }
}

pub fn format_absolute_path(path_buf: &Path) -> String {
    let path = format!("{}", path_buf.to_string_lossy());

    #[cfg(windows)]
    let path = path.replace("\\\\?\\", "");

    path
}

pub fn get_metadata(entry: &DirEntry, follow_symlinks: bool) -> Option<Metadata> {
    let metadata = match follow_symlinks {
        true => fs::metadata(entry.path()),
        false => symlink_metadata(entry.path()),
    };

    if let Ok(metadata) = metadata {
        return Some(metadata);
    }

    None
}


pub fn get_exif_metadata(entry: &DirEntry) -> Option<HashMap<String, String>> {
    if let Ok(file) = File::open(entry.path())
        && let Ok(reader) = exif::Reader::new().read_from_container(&mut BufReader::new(&file)) {
            let mut exif_info = HashMap::new();

            for field in reader.fields() {
                let field_tag = format!("{}", field.tag);
                match field.value {
                    exif::Value::Rational(ref vec)
                        if !vec.is_empty()
                            && (field_tag.eq("GPSLongitude")
                                || field_tag.eq("GPSLatitude")
                                || field_tag.eq("GPSAltitude")) =>
                    {
                        exif_info.insert(
                            field_tag,
                            vec.iter()
                                .map(|r| if r.denom != 0 { (r.num as f64 / r.denom as f64).to_string() } else { "0".to_string() })
                                .collect::<Vec<String>>()
                                .join(";"),
                        );
                    }
                    exif::Value::Ascii(ref vec) if !vec.is_empty() => {
                        if let Ok(str_value) = std::str::from_utf8(&vec[0]) {
                            exif_info.insert(field_tag, str_value.to_string());
                        }
                    }
                    _ => {
                        exif_info.insert(field_tag, field.value.display_as(field.tag).to_string());
                    }
                }
            }

            if let (Some(location), Some(location_ref)) =
                (exif_info.get("GPSLongitude").cloned(), exif_info.get("GPSLongitudeRef").cloned())
                && let Ok(coord) = parse_location_string(location, location_ref, "W") {
                    exif_info.insert(String::from("__Lng"), coord.to_string());
                }

            if let (Some(location), Some(location_ref)) =
                (exif_info.get("GPSLatitude").cloned(), exif_info.get("GPSLatitudeRef").cloned())
                && let Ok(coord) = parse_location_string(location, location_ref, "S") {
                    exif_info.insert(String::from("__Lat"), coord.to_string());
                }

            if let (Some(altitude_str), Some(altitude_ref)) =
                (exif_info.get("GPSAltitude").cloned(), exif_info.get("GPSAltitudeRef").cloned())
            {
                let mut altitude = altitude_str.parse::<f32>().unwrap_or(0.0);
                if altitude_ref.eq("1") {
                    altitude = -altitude;
                }
                exif_info.insert(String::from("__Alt"), altitude.to_string());
            }

            return Some(exif_info);
        }

    None
}

fn parse_location_string(s: String, location_ref: String, modifier_value: &str) -> Result<f32, ()> {
    let parts = s.split(';').map(|p| p.to_string()).collect::<Vec<String>>();
    if parts.len() == 3 {
        let mut coord = parts[0].parse::<f32>().unwrap_or(0.0)
            + parts[1].parse::<f32>().unwrap_or(0.0) / 60.0
            + parts[2].parse::<f32>().unwrap_or(0.0) / 3600.0;
        if location_ref.eq(modifier_value) {
            coord = -coord;
        }

        return Ok(coord);
    }

    Err(())
}

pub fn is_shebang(path: &PathBuf) -> bool {
    if let Ok(file) = File::open(path) {
        let mut buf_reader = BufReader::new(file);
        let mut buf = vec![0; 2];
        if buf_reader.read_exact(&mut buf).is_ok() {
            return buf[0] == 0x23 && buf[1] == 0x21;
        }
    }

    false
}

#[allow(unused)]
pub fn is_hidden(file_name: &str, metadata: &Option<Metadata>, archive_mode: bool) -> bool {
    if archive_mode {
        if !file_name.contains('\\') {
            let name = file_name.trim_end_matches('/');
            return parse_unix_filename(name).starts_with('.');
        } else {
            return false;
        }
    }

    #[cfg(unix)]
    {
        return file_name.starts_with('.');
    }

    #[cfg(windows)]
    {
        if let Some(metadata) = metadata {
            return mode::get_mode(metadata).contains("Hidden");
        }
    }

    #[cfg(not(unix))]
    {
        false
    }
}

pub fn get_line_count(entry: &DirEntry) -> Option<usize> {
    if let Ok(file) = File::open(entry.path()) {
        let mut reader = BufReader::with_capacity(1024 * 32, file);
        let mut count = 0;

        loop {
            let len = {
                if let Ok(buf) = reader.fill_buf() {
                    if buf.is_empty() {
                        break;
                    }

                    count += bytecount::count(buf, b'\n');
                    buf.len()
                } else {
                    return None;
                }
            };

            reader.consume(len);
        }

        return Some(count);
    }

    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentStats {
    pub is_text: bool,
    pub char_count: usize,
    pub word_count: usize,
    pub encoding: String,
    pub has_bom: bool,
    pub line_ending: String,
}

fn detect_bom(bytes: &[u8]) -> Option<(&'static str, usize)> {
    if bytes.starts_with(&[0xFF, 0xFE, 0x00, 0x00]) {
        Some(("UTF-32LE", 4))
    } else if bytes.starts_with(&[0x00, 0x00, 0xFE, 0xFF]) {
        Some(("UTF-32BE", 4))
    } else if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        Some(("UTF-8", 3))
    } else if bytes.starts_with(&[0xFF, 0xFE]) {
        Some(("UTF-16LE", 2))
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        Some(("UTF-16BE", 2))
    } else {
        None
    }
}

pub fn has_bom(entry: &DirEntry) -> bool {
    if let Ok(mut file) = File::open(entry.path()) {
        let mut buf = [0u8; 4];
        if let Ok(n) = file.read(&mut buf) {
            return detect_bom(&buf[..n]).is_some();
        }
    }
    false
}

// The whole-buffer decoders below are the reference implementation kept as a
// test oracle: the production path (`get_content_stats`) streams instead, and
// the parity tests assert the streaming results match these byte-for-byte.
#[cfg(test)]
fn decode_utf16(body: &[u8], big_endian: bool) -> String {
    let units = body.chunks_exact(2).map(|pair| {
        if big_endian {
            u16::from_be_bytes([pair[0], pair[1]])
        } else {
            u16::from_le_bytes([pair[0], pair[1]])
        }
    });
    char::decode_utf16(units)
        .map(|r| r.unwrap_or('\u{FFFD}'))
        .collect()
}

#[cfg(test)]
fn decode_utf32(body: &[u8], big_endian: bool) -> String {
    body.chunks_exact(4)
        .map(|q| {
            let value = if big_endian {
                u32::from_be_bytes([q[0], q[1], q[2], q[3]])
            } else {
                u32::from_le_bytes([q[0], q[1], q[2], q[3]])
            };
            char::from_u32(value).unwrap_or('\u{FFFD}')
        })
        .collect()
}

#[cfg(test)]
fn detect_line_ending(text: &str) -> String {
    let bytes = text.as_bytes();
    let (mut crlf, mut lf, mut cr) = (false, false, false);
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                if bytes.get(i + 1) == Some(&b'\n') {
                    crlf = true;
                    i += 2;
                    continue;
                }
                cr = true;
            }
            b'\n' => lf = true,
            _ => {}
        }
        i += 1;
    }
    match crlf as u8 + lf as u8 + cr as u8 {
        0 => String::new(),
        1 if crlf => String::from("CRLF"),
        1 if lf => String::from("LF"),
        1 => String::from("CR"),
        _ => String::from("Mixed"),
    }
}

#[cfg(test)]
fn compute_content_stats(bytes: &[u8]) -> ContentStats {
    let bom = detect_bom(bytes);
    let has_bom = bom.is_some();
    let (encoding, bom_len) = match bom {
        Some((name, len)) => (name.to_string(), len),
        None => (String::new(), 0),
    };
    let body = &bytes[bom_len..];

    let (encoding, is_text, text) = match encoding.as_str() {
        "UTF-16LE" => (encoding, true, decode_utf16(body, false)),
        "UTF-16BE" => (encoding, true, decode_utf16(body, true)),
        "UTF-32LE" => (encoding, true, decode_utf32(body, false)),
        "UTF-32BE" => (encoding, true, decode_utf32(body, true)),
        "UTF-8" => (encoding, true, String::from_utf8_lossy(body).into_owned()),
        _ if body.contains(&0u8) => (String::new(), false, String::new()),
        _ if body.is_ascii() => (String::from("ASCII"), true, String::from_utf8_lossy(body).into_owned()),
        _ => match std::str::from_utf8(body) {
            Ok(text) => (String::from("UTF-8"), true, text.to_string()),
            Err(_) => (String::from("ISO-8859-1"), true, body.iter().map(|&b| b as char).collect()),
        },
    };

    ContentStats {
        char_count: text.chars().count(),
        word_count: text.split_whitespace().count(),
        line_ending: detect_line_ending(&text),
        is_text,
        has_bom,
        encoding,
    }
}

pub fn get_content_stats(entry: &DirEntry) -> Option<ContentStats> {
    let file = File::open(entry.path()).ok()?;
    content_stats_from_reader(file)
}

/// Reusable scratch buffer size for streaming reads. Large enough to amortise
/// syscalls, small enough that peak memory stays bounded regardless of file
/// size — the whole point of streaming these stats rather than `fs::read`.
const CONTENT_CHUNK: usize = 32 * 1024;

/// Accumulates char/word/line-ending counts from a stream of decoded `char`s.
/// Char counting, word splitting (Unicode `White_Space`) and line-ending
/// detection are all derived from the same scalar stream, matching what the
/// in-memory oracle computes from a fully decoded `String`.
#[derive(Default)]
struct ContentScan {
    char_count: usize,
    word_count: usize,
    in_word: bool,
    crlf: bool,
    lf: bool,
    cr: bool,
    pending_cr: bool,
}

impl ContentScan {
    fn push(&mut self, c: char) {
        self.char_count += 1;

        if c.is_whitespace() {
            self.in_word = false;
        } else if !self.in_word {
            self.in_word = true;
            self.word_count += 1;
        }

        // Line endings: '\r' and '\n' are single ASCII scalars in every encoding
        // we decode, so scanning the char stream matches a byte scan exactly.
        if self.pending_cr {
            self.pending_cr = false;
            if c == '\n' {
                self.crlf = true;
                return;
            }
            self.cr = true;
        }
        if c == '\r' {
            self.pending_cr = true;
        } else if c == '\n' {
            self.lf = true;
        }
    }

    /// Resolve a trailing lone '\r' once the stream ends.
    fn finish_line(&mut self) {
        if self.pending_cr {
            self.cr = true;
            self.pending_cr = false;
        }
    }

    fn line_ending(&self) -> String {
        match self.crlf as u8 + self.lf as u8 + self.cr as u8 {
            0 => String::new(),
            1 if self.crlf => String::from("CRLF"),
            1 if self.lf => String::from("LF"),
            1 => String::from("CR"),
            _ => String::from("Mixed"),
        }
    }
}

/// Decode `buf` as strict UTF-8, pushing each scalar to `scan`. Returns
/// `Ok(n)` where `n` trailing bytes form an incomplete-but-still-valid sequence
/// to carry into the next chunk, or `Err(())` when `buf` contains a byte
/// sequence that can never be valid UTF-8 (caller falls back to Latin-1).
fn feed_utf8_strict(buf: &[u8], scan: &mut ContentScan) -> Result<usize, ()> {
    match std::str::from_utf8(buf) {
        Ok(s) => {
            for c in s.chars() {
                scan.push(c);
            }
            Ok(0)
        }
        Err(e) => {
            let valid_up_to = e.valid_up_to();
            if valid_up_to > 0
                && let Ok(s) = std::str::from_utf8(&buf[..valid_up_to]) {
                    for c in s.chars() {
                        scan.push(c);
                    }
                }
            match e.error_len() {
                None => Ok(buf.len() - valid_up_to),
                Some(_) => Err(()),
            }
        }
    }
}

/// Decode `buf` as UTF-8 with U+FFFD replacement (matching `from_utf8_lossy`),
/// pushing scalars to `scan`. Returns the number of trailing bytes forming an
/// incomplete sequence to carry; a tail still carried at EOF becomes one U+FFFD.
fn feed_utf8_lossy(buf: &[u8], scan: &mut ContentScan) -> usize {
    let mut pos = 0;
    loop {
        match std::str::from_utf8(&buf[pos..]) {
            Ok(s) => {
                for c in s.chars() {
                    scan.push(c);
                }
                return 0;
            }
            Err(e) => {
                let valid_up_to = e.valid_up_to();
                if valid_up_to > 0
                    && let Ok(s) = std::str::from_utf8(&buf[pos..pos + valid_up_to]) {
                        for c in s.chars() {
                            scan.push(c);
                        }
                    }
                pos += valid_up_to;
                match e.error_len() {
                    None => return buf.len() - pos,
                    Some(len) => {
                        scan.push('\u{FFFD}');
                        pos += len;
                    }
                }
            }
        }
    }
}

/// A streaming consumer of raw file bytes that yields the final `ContentStats`.
trait ContentSink {
    fn consume(&mut self, bytes: &[u8]);
    fn finish(self) -> ContentStats;
}

/// Drives a sink over the remaining reader, after an already-read `prefix`
/// (the bytes peeked for BOM detection that belong to the body). Returns `None`
/// on a read error, matching the previous `fs::read(...).ok()` behaviour.
fn drive_sink<R: Read, S: ContentSink>(
    mut reader: R,
    prefix: &[u8],
    mut sink: S,
) -> Option<ContentStats> {
    if !prefix.is_empty() {
        sink.consume(prefix);
    }
    let mut buf = [0u8; CONTENT_CHUNK];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => sink.consume(&buf[..n]),
            Err(_) => return None,
        }
    }
    Some(sink.finish())
}

fn read_up_to_4<R: Read>(reader: &mut R, buf: &mut [u8; 4]) -> Option<usize> {
    let mut filled = 0;
    while filled < 4 {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(_) => return None,
        }
    }
    Some(filled)
}

fn content_stats_from_reader<R: Read>(mut reader: R) -> Option<ContentStats> {
    let mut head = [0u8; 4];
    let head_len = read_up_to_4(&mut reader, &mut head)?;
    let head = &head[..head_len];

    match detect_bom(head) {
        Some(("UTF-8", len)) => drive_sink(reader, &head[len..], Utf8BomSink::default()),
        Some(("UTF-16LE", len)) => drive_sink(reader, &head[len..], Utf16Sink::new(false)),
        Some(("UTF-16BE", len)) => drive_sink(reader, &head[len..], Utf16Sink::new(true)),
        Some(("UTF-32LE", len)) => drive_sink(reader, &head[len..], Utf32Sink::new(false)),
        Some(("UTF-32BE", len)) => drive_sink(reader, &head[len..], Utf32Sink::new(true)),
        Some((_, _)) => None,
        None => drive_sink(reader, head, NoBomSink::new()),
    }
}

/// No-BOM body. The encoding can only be settled once the whole body is seen
/// (NUL ⇒ binary, all-ASCII ⇒ ASCII, valid ⇒ UTF-8, otherwise ⇒ Latin-1), so
/// the UTF-8 and Latin-1 interpretations are counted in parallel and the right
/// one is chosen in `finish`.
struct NoBomSink {
    utf8: ContentScan,
    latin1: ContentScan,
    utf8_valid: bool,
    utf8_carry: Vec<u8>,
    scratch: Vec<u8>,
    has_nul: bool,
    is_ascii: bool,
    byte_count: usize,
}

impl NoBomSink {
    fn new() -> Self {
        NoBomSink {
            utf8: ContentScan::default(),
            latin1: ContentScan::default(),
            utf8_valid: true,
            utf8_carry: Vec::new(),
            scratch: Vec::new(),
            has_nul: false,
            is_ascii: true,
            byte_count: 0,
        }
    }
}

impl ContentSink for NoBomSink {
    fn consume(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.byte_count += 1;
            if b == 0 {
                self.has_nul = true;
            }
            if b >= 0x80 {
                self.is_ascii = false;
            }
            self.latin1.push(b as char);
        }

        if !self.utf8_valid {
            return;
        }

        // Decode (carry + new bytes) as strict UTF-8. The carry holds an
        // incomplete sequence split across the previous chunk boundary; it was
        // already counted for Latin-1, so it is never re-fed there.
        if self.utf8_carry.is_empty() {
            match feed_utf8_strict(bytes, &mut self.utf8) {
                Ok(tail) => {
                    self.utf8_carry.clear();
                    self.utf8_carry.extend_from_slice(&bytes[bytes.len() - tail..]);
                }
                Err(()) => {
                    self.utf8_valid = false;
                    self.utf8_carry.clear();
                }
            }
        } else {
            self.scratch.clear();
            self.scratch.extend_from_slice(&self.utf8_carry);
            self.scratch.extend_from_slice(bytes);
            match feed_utf8_strict(&self.scratch, &mut self.utf8) {
                Ok(tail) => {
                    let start = self.scratch.len() - tail;
                    self.utf8_carry = self.scratch[start..].to_vec();
                }
                Err(()) => {
                    self.utf8_valid = false;
                    self.utf8_carry.clear();
                }
            }
        }
    }

    fn finish(mut self) -> ContentStats {
        self.latin1.finish_line();
        // A trailing incomplete sequence means the body is not valid UTF-8.
        let utf8_ok = self.utf8_valid && self.utf8_carry.is_empty();
        if utf8_ok {
            self.utf8.finish_line();
        }

        if self.has_nul {
            ContentStats {
                is_text: false,
                char_count: 0,
                word_count: 0,
                encoding: String::new(),
                has_bom: false,
                line_ending: String::new(),
            }
        } else if self.is_ascii {
            ContentStats {
                is_text: true,
                char_count: self.utf8.char_count,
                word_count: self.utf8.word_count,
                encoding: String::from("ASCII"),
                has_bom: false,
                line_ending: self.utf8.line_ending(),
            }
        } else if utf8_ok {
            ContentStats {
                is_text: true,
                char_count: self.utf8.char_count,
                word_count: self.utf8.word_count,
                encoding: String::from("UTF-8"),
                has_bom: false,
                line_ending: self.utf8.line_ending(),
            }
        } else {
            // Latin-1: one char per body byte.
            ContentStats {
                is_text: true,
                char_count: self.byte_count,
                word_count: self.latin1.word_count,
                encoding: String::from("ISO-8859-1"),
                has_bom: false,
                line_ending: self.latin1.line_ending(),
            }
        }
    }
}

/// UTF-8 body behind a BOM: decoded leniently (`from_utf8_lossy` semantics).
#[derive(Default)]
struct Utf8BomSink {
    scan: ContentScan,
    carry: Vec<u8>,
    scratch: Vec<u8>,
}

impl ContentSink for Utf8BomSink {
    fn consume(&mut self, bytes: &[u8]) {
        if self.carry.is_empty() {
            let tail = feed_utf8_lossy(bytes, &mut self.scan);
            self.carry.clear();
            self.carry.extend_from_slice(&bytes[bytes.len() - tail..]);
        } else {
            self.scratch.clear();
            self.scratch.extend_from_slice(&self.carry);
            self.scratch.extend_from_slice(bytes);
            let tail = feed_utf8_lossy(&self.scratch, &mut self.scan);
            let start = self.scratch.len() - tail;
            self.carry = self.scratch[start..].to_vec();
        }
    }

    fn finish(mut self) -> ContentStats {
        if !self.carry.is_empty() {
            self.scan.push('\u{FFFD}');
        }
        self.scan.finish_line();
        ContentStats {
            is_text: true,
            char_count: self.scan.char_count,
            word_count: self.scan.word_count,
            encoding: String::from("UTF-8"),
            has_bom: true,
            line_ending: self.scan.line_ending(),
        }
    }
}

/// UTF-16 body behind a BOM. Carries one odd byte across chunk boundaries and
/// one pending high surrogate; unpaired surrogates become U+FFFD, matching
/// `char::decode_utf16(..).unwrap_or('\u{FFFD}')`.
struct Utf16Sink {
    scan: ContentScan,
    big_endian: bool,
    byte_carry: Option<u8>,
    pending_high: Option<u16>,
}

impl Utf16Sink {
    fn new(big_endian: bool) -> Self {
        Utf16Sink {
            scan: ContentScan::default(),
            big_endian,
            byte_carry: None,
            pending_high: None,
        }
    }

    fn push_unit(&mut self, unit: u16) {
        if let Some(high) = self.pending_high.take() {
            if (0xDC00..=0xDFFF).contains(&unit) {
                let c = 0x10000 + (((high - 0xD800) as u32) << 10) + (unit - 0xDC00) as u32;
                self.scan.push(char::from_u32(c).unwrap_or('\u{FFFD}'));
                return;
            }
            self.scan.push('\u{FFFD}'); // unpaired high surrogate
        }

        if (0xD800..=0xDBFF).contains(&unit) {
            self.pending_high = Some(unit);
        } else if (0xDC00..=0xDFFF).contains(&unit) {
            self.scan.push('\u{FFFD}'); // unpaired low surrogate
        } else {
            self.scan.push(char::from_u32(unit as u32).unwrap_or('\u{FFFD}'));
        }
    }
}

impl ContentSink for Utf16Sink {
    fn consume(&mut self, bytes: &[u8]) {
        let mut idx = 0;
        if let Some(b0) = self.byte_carry.take() {
            let Some(&b1) = bytes.first() else {
                self.byte_carry = Some(b0);
                return;
            };
            idx = 1;
            let unit = if self.big_endian {
                u16::from_be_bytes([b0, b1])
            } else {
                u16::from_le_bytes([b0, b1])
            };
            self.push_unit(unit);
        }

        let mut chunks = bytes[idx..].chunks_exact(2);
        for pair in chunks.by_ref() {
            let unit = if self.big_endian {
                u16::from_be_bytes([pair[0], pair[1]])
            } else {
                u16::from_le_bytes([pair[0], pair[1]])
            };
            self.push_unit(unit);
        }
        if let Some(&b) = chunks.remainder().first() {
            self.byte_carry = Some(b);
        }
    }

    fn finish(mut self) -> ContentStats {
        if self.pending_high.is_some() {
            self.scan.push('\u{FFFD}');
        }
        // A trailing odd byte is dropped, matching the oracle's chunks_exact(2).
        self.scan.finish_line();
        ContentStats {
            is_text: true,
            char_count: self.scan.char_count,
            word_count: self.scan.word_count,
            encoding: String::from(if self.big_endian { "UTF-16BE" } else { "UTF-16LE" }),
            has_bom: true,
            line_ending: self.scan.line_ending(),
        }
    }
}

/// UTF-32 body behind a BOM. Carries up to three bytes across chunk boundaries;
/// out-of-range code points become U+FFFD, matching the oracle.
struct Utf32Sink {
    scan: ContentScan,
    big_endian: bool,
    byte_carry: Vec<u8>,
}

impl Utf32Sink {
    fn new(big_endian: bool) -> Self {
        Utf32Sink {
            scan: ContentScan::default(),
            big_endian,
            byte_carry: Vec::with_capacity(4),
        }
    }

    fn unit(&self, q: &[u8]) -> u32 {
        if self.big_endian {
            u32::from_be_bytes([q[0], q[1], q[2], q[3]])
        } else {
            u32::from_le_bytes([q[0], q[1], q[2], q[3]])
        }
    }
}

impl ContentSink for Utf32Sink {
    fn consume(&mut self, bytes: &[u8]) {
        let mut idx = 0;
        if !self.byte_carry.is_empty() {
            let need = 4 - self.byte_carry.len();
            let take = need.min(bytes.len());
            self.byte_carry.extend_from_slice(&bytes[..take]);
            idx = take;
            if self.byte_carry.len() < 4 {
                return;
            }
            let value = self.unit(&self.byte_carry);
            self.scan.push(char::from_u32(value).unwrap_or('\u{FFFD}'));
            self.byte_carry.clear();
        }

        let mut chunks = bytes[idx..].chunks_exact(4);
        for q in chunks.by_ref() {
            let value = self.unit(q);
            self.scan.push(char::from_u32(value).unwrap_or('\u{FFFD}'));
        }
        self.byte_carry.extend_from_slice(chunks.remainder());
    }

    fn finish(mut self) -> ContentStats {
        // Trailing bytes shorter than 4 are dropped, matching chunks_exact(4).
        self.scan.finish_line();
        ContentStats {
            is_text: true,
            char_count: self.scan.char_count,
            word_count: self.scan.word_count,
            encoding: String::from(if self.big_endian { "UTF-32BE" } else { "UTF-32LE" }),
            has_bom: true,
            line_ending: self.scan.line_ending(),
        }
    }
}

fn hash_file<D: sha1::Digest>(entry: &DirEntry) -> String {
    if let Ok(mut file) = File::open(entry.path()) {
        let mut hasher = D::new();
        let mut buf = [0u8; 8192];
        loop {
            match file.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => hasher.update(&buf[..n]),
                Err(_) => return String::new(),
            }
        }
        let hash = hasher.finalize();
        let mut hex = String::with_capacity(hash.len() * 2);
        for byte in hash.iter() {
            use std::fmt::Write;
            let _ = write!(hex, "{:02x}", byte);
        }
        return hex;
    }

    String::new()
}

pub fn get_sha1_file_hash(entry: &DirEntry) -> String {
    hash_file::<sha1::Sha1>(entry)
}

pub fn get_sha256_file_hash(entry: &DirEntry) -> String {
    hash_file::<sha2::Sha256>(entry)
}

pub fn get_sha512_file_hash(entry: &DirEntry) -> String {
    hash_file::<sha2::Sha512>(entry)
}

pub fn get_sha3_512_file_hash(entry: &DirEntry) -> String {
    hash_file::<sha3::Sha3_512>(entry)
}

pub fn is_dir_empty(entry: &DirEntry) -> Option<bool> {
    match fs::read_dir(entry.path()) {
        Ok(mut dir) => Some(dir.next().is_none()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::Field;

    #[test]
    fn topn_equal_criteria_keys_accumulate_values() {
        // The grouped-results path inserts every group under equal (possibly
        // empty) ordering criteria; none of them may be lost.
        let fields = Rc::new(vec![]);
        let ord = Rc::new(vec![]);
        let mut t: TopN<Criteria<String>, Vec<(String, String)>> = TopN::limitless();
        for i in 0..3 {
            t.insert(
                Criteria::new(fields.clone(), vec![], ord.clone()),
                vec![(format!("k{}", i), String::from("v"))],
            );
        }
        assert_eq!(t.iter_values().count(), 3);
    }

    #[test]
    fn test_content_stats_ascii() {
        let stats = compute_content_stats(b"hello world\nfoo bar baz\n");
        assert!(stats.is_text);
        assert_eq!(stats.encoding, "ASCII");
        assert!(!stats.has_bom);
        assert_eq!(stats.word_count, 5);
        assert_eq!(stats.char_count, 24);
        assert_eq!(stats.line_ending, "LF");
    }

    #[test]
    fn test_content_stats_utf8_multibyte_char_count() {
        // "héllo" + " wörld": accented chars are multi-byte in UTF-8, so the
        // character count must be lower than the byte length.
        let stats = compute_content_stats("héllo wörld".as_bytes());
        assert!(stats.is_text);
        assert_eq!(stats.encoding, "UTF-8");
        assert_eq!(stats.char_count, 11);
        assert_eq!(stats.word_count, 2);
    }

    #[test]
    fn test_content_stats_utf8_bom() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"abc");
        let stats = compute_content_stats(&bytes);
        assert!(stats.has_bom);
        assert_eq!(stats.encoding, "UTF-8");
        // The BOM is stripped before counting.
        assert_eq!(stats.char_count, 3);
    }

    #[test]
    fn test_content_stats_utf16le_bom() {
        // BOM + "Hi" in UTF-16LE.
        let bytes = [0xFF, 0xFE, b'H', 0x00, b'i', 0x00];
        let stats = compute_content_stats(&bytes);
        assert!(stats.is_text);
        assert!(stats.has_bom);
        assert_eq!(stats.encoding, "UTF-16LE");
        assert_eq!(stats.char_count, 2);
        assert_eq!(stats.word_count, 1);
    }

    #[test]
    fn test_content_stats_crlf_and_mixed() {
        assert_eq!(compute_content_stats(b"a\r\nb\r\n").line_ending, "CRLF");
        assert_eq!(compute_content_stats(b"a\rb\r").line_ending, "CR");
        assert_eq!(compute_content_stats(b"a\r\nb\nc").line_ending, "Mixed");
        assert_eq!(compute_content_stats(b"no newline").line_ending, "");
    }

    #[test]
    fn test_content_stats_binary() {
        // A NUL byte (without a UTF-16/32 BOM) marks the content as binary.
        let stats = compute_content_stats(&[0x00, 0x01, 0x02, b'a']);
        assert!(!stats.is_text);
        assert_eq!(stats.encoding, "");
    }

    #[test]
    fn test_content_stats_latin1_fallback() {
        // 0xE9 is "é" in Latin-1 but not valid UTF-8 on its own.
        let stats = compute_content_stats(&[b'c', b'a', b'f', 0xE9]);
        assert!(stats.is_text);
        assert_eq!(stats.encoding, "ISO-8859-1");
        assert_eq!(stats.char_count, 4);
    }

    /// Reader that hands out at most `chunk` bytes per `read`, to force the
    /// streaming decoders' carry/boundary logic to run across split sequences.
    struct ChunkReader<'a> {
        data: &'a [u8],
        pos: usize,
        chunk: usize,
    }

    impl<'a> ChunkReader<'a> {
        fn new(data: &'a [u8], chunk: usize) -> Self {
            ChunkReader { data, pos: 0, chunk }
        }
    }

    impl Read for ChunkReader<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let remaining = &self.data[self.pos..];
            let n = remaining.len().min(buf.len()).min(self.chunk);
            buf[..n].copy_from_slice(&remaining[..n]);
            self.pos += n;
            Ok(n)
        }
    }

    /// The streaming production path must produce exactly what the in-memory
    /// oracle does — read in one gulp, one byte at a time, and in 3-byte chunks
    /// (so multi-byte sequences straddle boundaries in three different ways).
    fn assert_stream_matches_oracle(bytes: &[u8]) {
        let oracle = compute_content_stats(bytes);

        let bulk = content_stats_from_reader(bytes).expect("bulk read");
        assert_eq!(bulk, oracle, "bulk streaming mismatch for {:?}", bytes);

        let one = content_stats_from_reader(ChunkReader::new(bytes, 1)).expect("1-byte read");
        assert_eq!(one, oracle, "1-byte streaming mismatch for {:?}", bytes);

        let three = content_stats_from_reader(ChunkReader::new(bytes, 3)).expect("3-byte read");
        assert_eq!(three, oracle, "3-byte streaming mismatch for {:?}", bytes);
    }

    #[test]
    fn streaming_matches_oracle_across_encodings() {
        let utf8_bom = |s: &str| {
            let mut v = vec![0xEF, 0xBB, 0xBF];
            v.extend_from_slice(s.as_bytes());
            v
        };
        let utf16le = |units: &[u16]| {
            let mut v = vec![0xFF, 0xFE];
            for u in units {
                v.extend_from_slice(&u.to_le_bytes());
            }
            v
        };
        let utf16be = |units: &[u16]| {
            let mut v = vec![0xFE, 0xFF];
            for u in units {
                v.extend_from_slice(&u.to_be_bytes());
            }
            v
        };
        let utf32le = |scalars: &[u32]| {
            let mut v = vec![0xFF, 0xFE, 0x00, 0x00];
            for u in scalars {
                v.extend_from_slice(&u.to_le_bytes());
            }
            v
        };
        let utf32be = |scalars: &[u32]| {
            let mut v = vec![0x00, 0x00, 0xFE, 0xFF];
            for u in scalars {
                v.extend_from_slice(&u.to_be_bytes());
            }
            v
        };

        let mut cases: Vec<Vec<u8>> = vec![
            // --- no BOM: ASCII ---
            b"".to_vec(),
            b"hello world\nfoo bar baz\n".to_vec(),
            b"a\r\nb\r\n".to_vec(),
            b"a\rb\r".to_vec(),
            b"a\r\nb\nc".to_vec(),
            b"no newline".to_vec(),
            b"trailing cr\r".to_vec(),
            b"  leading and trailing  ".to_vec(),
            b"tabs\tand\tspaces here".to_vec(),
            // --- no BOM: UTF-8 ---
            "héllo wörld".as_bytes().to_vec(),
            "smile \u{1F600} done".as_bytes().to_vec(),
            "ideographic\u{3000}space".as_bytes().to_vec(), // U+3000 is whitespace
            "mix\r\né\nb".as_bytes().to_vec(),
            // --- no BOM: Latin-1 fallback (invalid UTF-8, no NUL) ---
            vec![b'c', b'a', b'f', 0xE9],
            vec![b'a', 0xA0, b'b'],       // 0xA0 = NBSP, whitespace in Latin-1
            vec![0xE9, 0xE9, b' ', 0xFF], // all high bytes, invalid UTF-8
            // --- no BOM: binary (NUL present) ---
            vec![0x00, 0x01, 0x02, b'a'],
            b"hello\x00world".to_vec(),
            // --- UTF-8 BOM ---
            utf8_bom(""),
            utf8_bom("abc"),
            utf8_bom("héllo\r\nworld"),
            {
                // BOM + "é" + lone invalid byte → lossy U+FFFD
                let mut v = vec![0xEF, 0xBB, 0xBF, 0xC3, 0xA9, 0xFF];
                v.push(b'z');
                v
            },
            // --- UTF-16 LE/BE ---
            utf16le(&[]),
            utf16le(&[b'H' as u16, b'i' as u16]),
            utf16be(&[b'H' as u16, b'i' as u16]),
            utf16le(&[b'a' as u16, b'\r' as u16, b'\n' as u16, b'b' as u16]),
            utf16le(&[0xD83D, 0xDE00]), // U+1F600 surrogate pair
            utf16le(&[0xD83D]),         // lone high surrogate → U+FFFD
            utf16le(&[0xDE00]),         // lone low surrogate → U+FFFD
            {
                // UTF-16LE with a trailing odd byte that must be dropped
                let mut v = utf16le(&[b'A' as u16]);
                v.push(0x42);
                v
            },
            // --- UTF-32 LE/BE ---
            utf32le(&[]),
            utf32le(&[b'A' as u32, b'\n' as u32]),
            utf32be(&[b'A' as u32, b'\n' as u32]),
            utf32le(&[0x1F600]),   // astral scalar
            utf32le(&[0x0011_0000]), // out of range → U+FFFD
            {
                // UTF-32LE with trailing partial unit (3 bytes) to drop
                let mut v = utf32le(&[b'A' as u32]);
                v.extend_from_slice(&[0x10, 0x20, 0x30]);
                v
            },
            // --- BOM-only files ---
            vec![0xFF, 0xFE],             // UTF-16LE BOM only
            vec![0xFF, 0xFE, 0x00, 0x00], // UTF-32LE BOM only
            vec![0x00, 0x00, 0xFE, 0xFF], // UTF-32BE BOM only
        ];
        // A short 3-byte prefix of the UTF-16LE BOM-then-byte ambiguity.
        cases.push(vec![0xFF, 0xFE, 0x41]);

        for bytes in &cases {
            assert_stream_matches_oracle(bytes);
        }
    }

    #[test]
    fn streaming_large_ascii_counts_across_chunks() {
        // ~500 KB read in 7-byte slices: the 5-byte "word " pattern straddles
        // chunk boundaries, exercising the read loop many times.
        let mut data = Vec::new();
        for _ in 0..100_000 {
            data.extend_from_slice(b"word ");
        }
        let stats = content_stats_from_reader(ChunkReader::new(&data, 7)).unwrap();
        assert_eq!(stats.encoding, "ASCII");
        assert_eq!(stats.word_count, 100_000);
        assert_eq!(stats.char_count, 500_000);
        assert_eq!(stats.line_ending, "");
    }

    #[test]
    fn streaming_multibyte_one_byte_at_a_time() {
        // Every "é" is 2 bytes; reading 1 byte per call forces the UTF-8 carry
        // path to span a boundary on every single character.
        let mut data = Vec::new();
        for _ in 0..1000 {
            data.extend_from_slice("é".as_bytes());
        }
        let stats = content_stats_from_reader(ChunkReader::new(&data, 1)).unwrap();
        assert_eq!(stats.encoding, "UTF-8");
        assert_eq!(stats.char_count, 1000);
        assert_eq!(stats.word_count, 1);
    }

    fn basic_criteria<T: Ord + Clone + Display>(vals: &[T]) -> Criteria<T> {
        let fields = Rc::new(vec![Expr::field(Field::Size); vals.len()]);
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
        let fields = Rc::new(vec![Expr::field(Field::Size); 3]);
        let orderings = Rc::new(vec![false, false, false]);

        let c1 = Criteria::new(fields.clone(), vec![1, 2, 3], orderings.clone());
        let c2 = Criteria::new(fields.clone(), vec![1, 3, 1], orderings.clone());

        assert_eq!(c1.cmp(&c2), Ordering::Greater);
    }

    #[test]
    fn test_compare_some_fields_reverse() {
        let fields = Rc::new(vec![Expr::field(Field::Size); 3]);
        let orderings = Rc::new(vec![true, false, true]);

        let c1 = Criteria::new(fields.clone(), vec![1, 2, 3], orderings.clone());
        let c2 = Criteria::new(fields.clone(), vec![1, 3, 1], orderings.clone());

        assert_eq!(c1.cmp(&c2), Ordering::Greater);
    }

    #[test]
    fn test_partial_cmp_consistent_with_cmp() {
        let c1 = basic_criteria(&[1, 3, 2]);
        let c2 = basic_criteria(&[1, 2, 3]);

        assert_eq!(c1.partial_cmp(&c2), Some(c1.cmp(&c2)));
    }

    #[test]
    fn test_parse_filesize() {
        let file_size = "abc";
        assert_eq!(parse_filesize(file_size), None);

        let file_size = "b";
        assert_eq!(parse_filesize(file_size), None);

        let file_size = "kb";
        assert_eq!(parse_filesize(file_size), None);

        let file_size = "gib";
        assert_eq!(parse_filesize(file_size), None);

        let file_size = " gibb";
        assert_eq!(parse_filesize(file_size), None);

        let file_size = "b123";
        assert_eq!(parse_filesize(file_size), None);

        let file_size = "123";
        assert_eq!(parse_filesize(file_size), Some(123));

        let file_size = "123b";
        assert_eq!(parse_filesize(file_size), Some(123));

        let file_size = "123 b";
        assert_eq!(parse_filesize(file_size), Some(123));

        let file_size = "1kb";
        assert_eq!(parse_filesize(file_size), Some(1000));

        let file_size = "1 kb";
        assert_eq!(parse_filesize(file_size), Some(1000));

        let file_size = "1kib";
        assert_eq!(parse_filesize(file_size), Some(1024));

        let file_size = "1 kib";
        assert_eq!(parse_filesize(file_size), Some(1024));
    }

    #[test]
    fn test_format_filesize() {
        let file_size = 1678123;

        assert_eq!(format_filesize(file_size, "").unwrap(), String::from("1.60MiB"));
        assert_eq!(format_filesize(file_size, " ").unwrap(), String::from("1.60 MiB"));
        assert_eq!(format_filesize(file_size, "%.0").unwrap(), String::from("2MiB"));
        assert_eq!(format_filesize(file_size, "%.1").unwrap(), String::from("1.6MiB"));
        assert_eq!(format_filesize(file_size, "%.2").unwrap(), String::from("1.60MiB"));
        assert_eq!(format_filesize(file_size, "%.2 ").unwrap(), String::from("1.60 MiB"));
        assert_eq!(format_filesize(file_size, "%.2 d").unwrap(), String::from("1.68 MB"));
        assert_eq!(format_filesize(file_size, "%.2 c").unwrap(), String::from("1.60 MB"));
        assert_eq!(
            format_filesize(file_size, "%.2 k").unwrap(),
            String::from("1638.79 KiB")
        );
        assert_eq!(
            format_filesize(file_size, "%.2 ck").unwrap(),
            String::from("1638.79 KB")
        );
        assert_eq!(
            format_filesize(file_size, "%.0 ck").unwrap(),
            String::from("1639 KB")
        );
        assert_eq!(
            format_filesize(file_size, "%.0 kb").unwrap(),
            String::from("1678 KB")
        );
        assert_eq!(format_filesize(file_size, "%.0kb").unwrap(), String::from("1678KB"));
        assert_eq!(format_filesize(file_size, "%.0s").unwrap(), String::from("2M"));
        assert_eq!(format_filesize(file_size, "%.0 s").unwrap(), String::from("2 M"));
    }

    #[test]
    fn test_format_filesize_large_precision() {
        // Should not panic on absurdly large precision values
        let result = format_filesize(1024, "%.9999999999");
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_extension() {
        assert_eq!(get_extension(".no_ext"), String::new());
        assert_eq!(get_extension("no_ext"), String::new());
        assert_eq!(get_extension("has_ext.foo"), String::from("foo"));
        assert_eq!(get_extension("has_ext.foobar"), String::from("foobar"));
        assert_eq!(get_extension("has.extension.foo"), String::from("foo"));
    }

    #[test]
    fn test_parse_unix_filename() {
        assert_eq!(parse_unix_filename("file.txt"), "file.txt");
        assert_eq!(parse_unix_filename("path/to/file.txt"), "file.txt");
        assert_eq!(parse_unix_filename("path/to/.hidden"), ".hidden");
        assert_eq!(parse_unix_filename("/root.txt"), "root.txt");
    }

    #[test]
    fn test_is_hidden_archive_trailing_slash() {
        // Archive directory entries often have trailing slashes
        assert!(is_hidden(".hidden/", &None, true));
        assert!(is_hidden("path/to/.hidden/", &None, true));
        assert!(!is_hidden("path/to/visible/", &None, true));
        assert!(is_hidden(".hidden", &None, true));
    }

    #[test]
    fn test_gps_rational_zero_denom() {
        // Simulates the GPS rational conversion logic
        let convert = |num: u32, denom: u32| -> String {
            if denom != 0 { (num as f64 / denom as f64).to_string() } else { "0".to_string() }
        };
        assert_eq!(convert(180, 1), "180");
        assert_eq!(convert(0, 0), "0");
        assert_eq!(convert(90, 0), "0");
    }

    #[test]
    fn test_capitalize() {
        assert_eq!(capitalize_initials(""), String::new());
        assert_eq!(capitalize_initials("test"), String::from("Test"));
        assert_eq!(capitalize_initials("some test"), String::from("Some Test"));
        assert_eq!(capitalize_initials("превед медвед"), String::from("Превед Медвед"));
    }

    fn dir_entry_for(path: &Path) -> DirEntry {
        let parent = path.parent().unwrap();
        let target = path.file_name().unwrap();
        fs::read_dir(parent)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_name() == target)
            .unwrap()
    }

    #[test]
    fn is_dir_empty_distinguishes_empty_and_non_empty() {
        let base = std::env::temp_dir().join("fselect_is_dir_empty_test");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();

        let empty = base.join("empty_dir");
        fs::create_dir_all(&empty).unwrap();
        let non_empty = base.join("non_empty_dir");
        fs::create_dir_all(&non_empty).unwrap();
        fs::write(non_empty.join("marker.txt"), b"x").unwrap();

        assert_eq!(is_dir_empty(&dir_entry_for(&empty)), Some(true));
        assert_eq!(is_dir_empty(&dir_entry_for(&non_empty)), Some(false));

        let _ = fs::remove_dir_all(&base);
    }

    fn numeric_criteria(value: &str, asc: bool) -> Criteria<String> {
        let fields = Rc::new(vec![Expr::field(Field::Size)]);
        let orderings = Rc::new(vec![asc]);
        Criteria::new(fields, vec![String::from(value)], orderings)
    }

    #[test]
    fn numeric_criteria_compares_fractional_values() {
        // Regression: fractional sort keys (e.g. `order by size / 1024`) used
        // to fall through the u64-only parser and collapse to 0.
        let a = numeric_criteria("5.875", true);
        let b = numeric_criteria("31.6875", true);
        assert_eq!(a.cmp(&b), Ordering::Less);
        assert_eq!(b.cmp(&a), Ordering::Greater);
    }

    #[test]
    fn numeric_criteria_compares_negative_values() {
        let a = numeric_criteria("-5", true);
        let b = numeric_criteria("3", true);
        assert_eq!(a.cmp(&b), Ordering::Less);
    }

    #[test]
    fn numeric_criteria_still_supports_size_suffixes() {
        let a = numeric_criteria("512", true);
        let b = numeric_criteria("1k", true);
        assert_eq!(a.cmp(&b), Ordering::Less, "1k must compare as 1024");
    }

    #[test]
    fn numeric_criteria_sorts_numbers_before_non_numbers() {
        let number = numeric_criteria("10", true);
        let empty = numeric_criteria("", true);
        assert_eq!(number.cmp(&empty), Ordering::Less);
        assert_eq!(empty.cmp(&number), Ordering::Greater);
    }

    #[test]
    fn datetime_values_under_numeric_expr_sort_chronologically() {
        // `order by max(modified)`: MAX is a numeric function, so the numeric
        // comparator is chosen, but the values are fixed-width datetimes.
        // They must sort by string order (== chronological), not tie at 0.
        use crate::function::Function;

        let max_modified = Expr::function_left(
            Function::Max,
            Some(Box::new(Expr::field(Field::Modified))),
        );
        let fields = Rc::new(vec![max_modified]);

        let make = |value: &str, asc: bool| {
            Criteria::new(
                fields.clone(),
                vec![String::from(value)],
                Rc::new(vec![asc]),
            )
        };

        let older = make("2024-01-05 09:30:00", true);
        let newer = make("2026-06-11 10:00:00", true);
        assert_eq!(older.cmp(&newer), Ordering::Less);
        assert_eq!(newer.cmp(&older), Ordering::Greater);

        // Descending must invert the comparison instead of being a no-op.
        let older_desc = make("2024-01-05 09:30:00", false);
        let newer_desc = make("2026-06-11 10:00:00", false);
        assert_eq!(older_desc.cmp(&newer_desc), Ordering::Greater);
    }

    #[test]
    fn datetime_fallback_is_epoch_with_zero_nanos() {
        // The unparseable-value fallback in cmp_at_datetimes should be exactly
        // 1970-01-01 00:00:00, not 1970-01-01 00:00:00.<current nanos>.
        // We verify this indirectly: an unparseable value should compare Equal
        // to a parseable "1970-01-01 00:00:00".
        let fields = Rc::new(vec![Expr::field(Field::Modified)]);
        let orderings = Rc::new(vec![true]);
        let parseable = Criteria::new(
            fields.clone(),
            vec![String::from("1970-01-01 00:00:00")],
            orderings.clone(),
        );
        let unparseable = Criteria::new(
            fields,
            vec![String::from("not_a_date")],
            orderings,
        );
        assert_eq!(parseable.cmp(&unparseable), Ordering::Equal);
    }

    #[test]
    fn str_to_bool_accepts_all_aliases() {
        for v in ["true", "t", "1", "yes", "y", "on", "TRUE", "T", "Yes", "ON"] {
            assert_eq!(str_to_bool(v), Some(true), "expected {} to be true", v);
        }
        for v in ["false", "f", "0", "no", "n", "off", "FALSE", "F", "No", "OFF"] {
            assert_eq!(str_to_bool(v), Some(false), "expected {} to be false", v);
        }
    }

    #[test]
    fn str_to_bool_rejects_unknown() {
        for v in ["", "maybe", "2", "tt", "ff", "truee"] {
            assert_eq!(str_to_bool(v), None, "expected {} to be unrecognized", v);
        }
    }
}
