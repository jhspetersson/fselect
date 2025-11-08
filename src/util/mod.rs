#[cfg(target_os = "linux")]
pub(crate) mod capabilities;
mod datetime;
pub mod dimensions;
pub mod duration;
mod glob;
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
use std::io;
use std::io::Read;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::LazyLock;

use chrono::{Datelike, Local, Timelike};
use mp3_metadata::MP3Metadata;
use regex::Regex;
use sha1::Digest;

pub use self::datetime::format_date;
pub use self::datetime::format_datetime;
pub use self::datetime::parse_datetime;
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd)]
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
        let a = parse_filesize(&self.values[i].to_string()).unwrap_or(0);
        let b = parse_filesize(&other.values[i].to_string()).unwrap_or(0);

        a.cmp(&b)
    }

    #[inline]
    fn cmp_at_datetimes(&self, other: &Self, i: usize) -> Ordering
    where
        T: Ord,
    {
        let default = Local::now()
            .naive_local()
            .with_year(1970)
            .unwrap()
            .with_month(1)
            .unwrap()
            .with_day(1)
            .unwrap()
            .with_hour(0)
            .unwrap()
            .with_minute(0)
            .unwrap()
            .with_second(0)
            .unwrap();
        let a = parse_datetime(&self.values[i].to_string())
            .unwrap_or((default, default))
            .0;
        let b = parse_datetime(&other.values[i].to_string())
            .unwrap_or((default, default))
            .0;

        a.cmp(&b)
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

#[cfg(windows)]
pub fn calc_depth(s: &str) -> u32 {
    s.matches("\\").count() as u32
}

#[cfg(not(windows))]
pub fn calc_depth(s: &str) -> u32 {
    s.matches("/").count() as u32
}

pub fn path_error_message(p: &Path, e: io::Error) {
    error_message(&p.to_string_lossy(), &e.to_string());
}

pub fn error_message(source: &str, description: &str) {
    eprint!("{}: {}", source, description);
}

pub fn error_exit(source: &str, description: &str) -> ! {
    error_message(source, description);
    eprintln!();
    std::process::exit(2);
}

pub fn get_extension(s: &str) -> String {
    match Path::new(s).extension() {
        Some(ext) => ext.to_string_lossy().to_string(),
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
            Ok(size) => Some(size * 1),
            _ => None,
        };
    }

    string.parse::<u64>().ok()
}

static FILE_SIZE_FORMAT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("(%\\.(?P<zeroes>\\d+))?(?P<space>\\s)?(?P<units>\\w+)?").unwrap()
});

pub fn format_filesize(size: u64, modifier: &str) -> String {
    let mut modifier = modifier.to_ascii_lowercase();

    let mut zeroes = -1;
    let mut space = false;

    if let Some(cap) = FILE_SIZE_FORMAT_REGEX.captures(&modifier) {
        zeroes = cap
            .name("zeroes")
            .map_or(-1, |m| m.as_str().parse::<i32>().unwrap());
        space = cap.name("space").map_or(false, |m| m.as_str() == " ");
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
        _ => error_exit("Unknown file size modifier", modifier.as_str()),
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

    result
}

pub fn str_to_bool(val: &str) -> Option<bool> {
    let str_val = val.to_ascii_lowercase();
    match str_val.as_str() {
        "true" | "1" | "yes" | "y" => Some(true),
        "false" | "0" | "no" | "n" => Some(false),
        _ => None,
    }
}

pub fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

pub fn parse_unix_filename(s: &str) -> &str {
    let last_slash = s.rfind('/');
    match last_slash {
        Some(idx) => &s[idx..],
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
        Err(err) => match err.to_string().starts_with("Incorrect function.") {
            true => Ok(format_absolute_path(path_buf)),
            _ => Err(err.to_string()),
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
        false => entry.metadata(),
        true => symlink_metadata(entry.path()),
    };

    if let Ok(metadata) = metadata {
        return Some(metadata);
    }

    None
}

pub fn get_mp3_metadata(entry: &DirEntry) -> Option<MP3Metadata> {
    match mp3_metadata::read_from_file(entry.path()) {
        Ok(mp3_meta) => Some(mp3_meta),
        _ => None,
    }
}

pub fn get_exif_metadata(entry: &DirEntry) -> Option<HashMap<String, String>> {
    if let Ok(file) = File::open(entry.path()) {
        if let Ok(reader) = exif::Reader::new().read_from_container(&mut BufReader::new(&file)) {
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
                                .map(|r| (r.num / r.denom).to_string())
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

            if exif_info.contains_key("GPSLongitude") && exif_info.contains_key("GPSLongitudeRef") {
                let location = exif_info.get("GPSLongitude").unwrap().to_string();
                let location_ref = exif_info.get("GPSLongitudeRef").unwrap().to_string();
                if let Ok(coord) = parse_location_string(location, location_ref, "W") {
                    exif_info.insert(String::from("__Lng"), coord.to_string());
                }
            }

            if exif_info.contains_key("GPSLatitude") && exif_info.contains_key("GPSLatitudeRef") {
                let location = exif_info.get("GPSLatitude").unwrap().to_string();
                let location_ref = exif_info.get("GPSLatitudeRef").unwrap().to_string();
                if let Ok(coord) = parse_location_string(location, location_ref, "S") {
                    exif_info.insert(String::from("__Lat"), coord.to_string());
                }
            }

            if exif_info.contains_key("GPSAltitude") && exif_info.contains_key("GPSAltitudeRef") {
                let mut altitude = exif_info
                    .get("GPSAltitude")
                    .unwrap()
                    .to_string()
                    .parse::<f32>()
                    .unwrap_or(0.0);
                let altitude_ref = exif_info.get("GPSAltitudeRef").unwrap().to_string();
                if altitude_ref.eq("1") {
                    altitude = -altitude;
                }
                exif_info.insert(String::from("__Alt"), altitude.to_string());
            }

            return Some(exif_info);
        }
    }

    None
}

fn parse_location_string(s: String, location_ref: String, modifier_value: &str) -> Result<f32, ()> {
    let parts = s.split(';').map(|p| p.to_string()).collect::<Vec<String>>();
    if parts.len() == 3 {
        let mut coord = parts[0].parse::<f32>().unwrap_or(0.0)
            + parts[1].parse::<f32>().unwrap_or(0.0) / 60.0
            + parts[2].parse::<f32>().unwrap_or(0.0) / 3660.0;
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
            return parse_unix_filename(file_name).starts_with('.');
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

pub fn get_sha1_file_hash(entry: &DirEntry) -> String {
    if let Ok(mut file) = File::open(entry.path()) {
        let mut hasher = sha1::Sha1::new();
        if io::copy(&mut file, &mut hasher).is_ok() {
            let hash = hasher.finalize();
            return format!("{:x}", hash);
        }
    }

    String::new()
}

pub fn get_sha256_file_hash(entry: &DirEntry) -> String {
    if let Ok(mut file) = File::open(entry.path()) {
        let mut hasher = sha2::Sha256::new();
        if io::copy(&mut file, &mut hasher).is_ok() {
            let hash = hasher.finalize();
            return format!("{:x}", hash);
        }
    }

    String::new()
}

pub fn get_sha512_file_hash(entry: &DirEntry) -> String {
    if let Ok(mut file) = File::open(entry.path()) {
        let mut hasher = sha2::Sha512::new();
        if io::copy(&mut file, &mut hasher).is_ok() {
            let hash = hasher.finalize();
            return format!("{:x}", hash);
        }
    }

    String::new()
}

pub fn get_sha3_512_file_hash(entry: &DirEntry) -> String {
    if let Ok(mut file) = File::open(entry.path()) {
        let mut hasher = sha3::Sha3_512::new();
        if io::copy(&mut file, &mut hasher).is_ok() {
            let hash = hasher.finalize();
            return format!("{:x}", hash);
        }
    }

    String::new()
}

pub fn is_dir_empty(entry: &DirEntry) -> Option<bool> {
    match fs::read_dir(entry.path()) {
        Ok(dir) => Some(!dir.into_iter().any(|_| true)),
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

        assert_eq!(format_filesize(file_size, ""), String::from("1.60MiB"));
        assert_eq!(format_filesize(file_size, " "), String::from("1.60 MiB"));
        assert_eq!(format_filesize(file_size, "%.0"), String::from("2MiB"));
        assert_eq!(format_filesize(file_size, "%.1"), String::from("1.6MiB"));
        assert_eq!(format_filesize(file_size, "%.2"), String::from("1.60MiB"));
        assert_eq!(format_filesize(file_size, "%.2 "), String::from("1.60 MiB"));
        assert_eq!(format_filesize(file_size, "%.2 d"), String::from("1.68 MB"));
        assert_eq!(format_filesize(file_size, "%.2 c"), String::from("1.60 MB"));
        assert_eq!(
            format_filesize(file_size, "%.2 k"),
            String::from("1638.79 KiB")
        );
        assert_eq!(
            format_filesize(file_size, "%.2 ck"),
            String::from("1638.79 KB")
        );
        assert_eq!(
            format_filesize(file_size, "%.0 ck"),
            String::from("1639 KB")
        );
        assert_eq!(
            format_filesize(file_size, "%.0 kb"),
            String::from("1678 KB")
        );
        assert_eq!(format_filesize(file_size, "%.0kb"), String::from("1678KB"));
        assert_eq!(format_filesize(file_size, "%.0s"), String::from("2M"));
        assert_eq!(format_filesize(file_size, "%.0 s"), String::from("2 M"));
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
    fn test_capitalize() {
        assert_eq!(capitalize(""), String::new());
        assert_eq!(capitalize("test"), String::from("Test"));
        assert_eq!(capitalize("some test"), String::from("Some test"));
        assert_eq!(capitalize("превед медвед"), String::from("Превед медвед"));
    }
}
