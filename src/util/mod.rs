#[cfg(target_os = "linux")]
pub(crate) mod acl;
pub(crate) mod app_dirs;
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
use mp3_metadata::MP3Metadata;
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

pub fn get_mp3_metadata(entry: &DirEntry) -> Option<MP3Metadata> {
    mp3_metadata::read_from_file(entry.path()).ok()
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
