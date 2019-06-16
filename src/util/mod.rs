mod datetime;
mod glob;
mod top_n;
mod wbuf;

use std::cmp::Ordering;
use std::collections::HashMap;
use std::error::Error;
use std::fmt::Display;
use std::fs;
use std::fs::canonicalize;
use std::fs::DirEntry;
use std::fs::File;
use std::fs::Metadata;
use std::fs::symlink_metadata;
use std::io;
use std::io::BufReader;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::string::ToString;

use chrono::Local;
use chrono::TimeZone;
use imagesize;
use mp3_metadata;
use mp3_metadata::MP3Metadata;
use sha1::Digest;

use crate::expr::Expr;
#[cfg(windows)]
use crate::mode;
pub use self::datetime::parse_datetime;
pub use self::datetime::to_local_datetime;
pub use self::datetime::format_datetime;
pub use self::glob::convert_glob_to_pattern;
pub use self::glob::convert_like_to_pattern;
pub use self::glob::is_glob;
pub use self::top_n::TopN;
pub use self::wbuf::WritableBuffer;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd)]
pub struct Criteria<T> where T: Display + ToString {
    fields: Rc<Vec<Expr>>,
    /// Values of current row to sort with, placed in order of significance.
    values: Vec<T>,
    /// Shared smart reference to Vector of boolean where each index corresponds to whether the
    /// field at that index should be ordered in ascending order `true` or descending order `false`.
    orderings: Rc<Vec<bool>>,
}

impl<T> Criteria<T> where T: Display {
    pub fn new(fields: Rc<Vec<Expr>>, values: Vec<T>, orderings: Rc<Vec<bool>>) -> Criteria<T> {
        debug_assert_eq!(fields.len(), values.len());
        debug_assert_eq!(values.len(), orderings.len());

        Criteria { fields, values, orderings }
    }

    #[inline]
    fn cmp_at(&self, other: &Self, i: usize) -> Ordering where T: Ord {
        let field = &self.fields[i];
        let comparison;
        if field.contains_numeric() {
            comparison = self.cmp_at_numbers(other, i);
        } else if field.contains_datetime() {
            comparison = self.cmp_at_datetimes(other, i);
        } else {
            comparison = self.cmp_at_direct(other, i);
        }

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

pub fn path_error_message(p: &Path, e: io::Error) {
    error_message(&p.to_string_lossy(), e.description());
}

pub fn error_message(source: &str, description: &str) {
    eprint!("{}: {}", source, description);
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

pub fn has_extension(file_name: &str, extensions: &[&str]) -> bool {
    let s = file_name.to_ascii_lowercase();

    for ext in extensions {
        if s.ends_with(ext) {
            return true
        }
    }

    false
}

pub fn is_text_mime(mime: &str) -> bool {
    mime.starts_with("text/") ||
    mime.contains("+xml") ||
    mime.contains("-xml") ||
    mime.eq("application/x-awk") ||
    mime.eq("application/x-perl") ||
    mime.eq("application/x-php") ||
    mime.eq("application/x-ruby") ||
    mime.eq("application/x-shellscript")
}

pub fn canonical_path(path_buf: &PathBuf) -> Result<String, ()> {
    if let Ok(path) = canonicalize(path_buf) {
        return Ok(format_absolute_path(&path));
    }

    Err(())
}

pub fn format_absolute_path(path_buf: &PathBuf) -> String {
    let path = format!("{}", path_buf.to_string_lossy());

    #[cfg(windows)]
    let path = path.replace("\\\\?\\", "");

    path
}

pub fn get_metadata(entry: &DirEntry, follow_symlinks: bool) -> Option<Metadata> {
    let metadata = match follow_symlinks {
        false => symlink_metadata(entry.path()),
        true => fs::metadata(entry.path())
    };

    if let Ok(metadata) = metadata {
        return Some(metadata);
    }

    None
}

fn is_image_dim_readable(file_name: &str) -> bool {
    let extensions = [".bmp", ".gif", ".jpeg", ".jpg", ".png", ".psb", ".psd", ".tiff", ".webp"];

    has_extension(file_name, &extensions)
}

pub fn get_img_dimensions(entry: &DirEntry) -> Option<(usize, usize)> {
    if !is_image_dim_readable(&entry.file_name().to_string_lossy()) {
        return None;
    }

    match imagesize::size(entry.path()) {
        Ok(dimensions) => Some((dimensions.width, dimensions.height)),
        _ => None
    }
}

pub fn get_mp3_metadata(entry: &DirEntry) -> Option<MP3Metadata> {
    match mp3_metadata::read_from_file(entry.path()) {
        Ok(mp3_meta) => Some(mp3_meta),
        _ => None
    }
}

pub fn get_exif_metadata(entry: &DirEntry) -> Option<HashMap<String, String>> {
    if let Ok(file) = File::open(entry.path()) {
        if let Ok(reader) = exif::Reader::new(&mut BufReader::new(&file)) {
            let mut exif_info = HashMap::new();

            for field in reader.fields().iter() {
                let field_value = match field.value {
                    exif::Value::Ascii(ref vec) if !vec.is_empty() => std::str::from_utf8(vec[0]).unwrap().to_string(),
                    _ => field.value.display_as(field.tag).to_string()
                };

                exif_info.insert(format!("{}", field.tag), field_value);
            }

            return Some(exif_info);
        }
    }

    None
}

pub fn is_shebang(path: &PathBuf) -> bool {
    if let Ok(file) = File::open(path) {
        let mut buf_reader = BufReader::new(file);
        let mut buf = vec![0; 2];
        if buf_reader.read_exact(&mut buf).is_ok() {
            return buf[0] == 0x23 && buf[1] == 0x21
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
            if let Some(ref metadata) = metadata {
                return mode::get_mode(metadata).contains("Hidden");
            }
        }

    #[cfg(not(unix))]
        {
            false
        }
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

pub fn get_sha3_512_file_hash(entry: &DirEntry) -> String {
    if let Ok(mut file) = File::open(&entry.path()) {
        let mut hasher = sha3::Sha3_512::new();
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
}
