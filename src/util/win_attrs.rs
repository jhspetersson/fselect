//! Windows file attribute detection, the closest equivalent to Linux
//! extended file flags (the `chattr`/`lsattr` flags exposed on Linux via
//! `FS_IOC_GETFLAGS`).
//!
//! Each NTFS file attribute is mapped to a single letter, mirroring the
//! letter-based formatting used by the Linux `extattrs` module so the
//! `extattrs` / `has_extattrs` / `has_extattr` query fields behave
//! consistently across platforms.

use std::fs::Metadata;
use std::os::windows::fs::MetadataExt;

const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x20;
const FILE_ATTRIBUTE_TEMPORARY: u32 = 0x100;
const FILE_ATTRIBUTE_SPARSE_FILE: u32 = 0x200;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const FILE_ATTRIBUTE_COMPRESSED: u32 = 0x800;
const FILE_ATTRIBUTE_OFFLINE: u32 = 0x1000;
const FILE_ATTRIBUTE_NOT_CONTENT_INDEXED: u32 = 0x2000;
const FILE_ATTRIBUTE_ENCRYPTED: u32 = 0x4000;
const FILE_ATTRIBUTE_INTEGRITY_STREAM: u32 = 0x8000;

/// Mapping of NTFS file attributes to single-letter flags. The letters follow
/// the conventions of the Windows `attrib` command where applicable
/// (R/A/S/H/I), with additional letters for the remaining attributes.
const FLAG_LETTERS: &[(u32, char)] = &[
    (FILE_ATTRIBUTE_READONLY, 'R'),
    (FILE_ATTRIBUTE_HIDDEN, 'H'),
    (FILE_ATTRIBUTE_SYSTEM, 'S'),
    (FILE_ATTRIBUTE_ARCHIVE, 'A'),
    (FILE_ATTRIBUTE_TEMPORARY, 'T'),
    (FILE_ATTRIBUTE_SPARSE_FILE, 'P'),
    (FILE_ATTRIBUTE_REPARSE_POINT, 'L'),
    (FILE_ATTRIBUTE_COMPRESSED, 'C'),
    (FILE_ATTRIBUTE_OFFLINE, 'O'),
    (FILE_ATTRIBUTE_NOT_CONTENT_INDEXED, 'I'),
    (FILE_ATTRIBUTE_ENCRYPTED, 'E'),
    (FILE_ATTRIBUTE_INTEGRITY_STREAM, 'V'),
];

/// Returns the raw NTFS file attribute bitmask for the given metadata.
pub fn get_attrs(meta: &Metadata) -> u32 {
    meta.file_attributes()
}

/// Formats the mapped file attributes as a string of single-letter flags,
/// e.g. `"RA"` for a read-only file with the archive bit set. Returns an
/// empty string if none of the mapped attributes are set.
pub fn format_attrs(attrs: u32) -> String {
    let mut result = String::new();
    for &(flag, letter) in FLAG_LETTERS {
        if attrs & flag != 0 {
            result.push(letter);
        }
    }
    result
}

/// Returns `true` if any of the mapped attributes is set.
pub fn has_any_attr(attrs: u32) -> bool {
    FLAG_LETTERS.iter().any(|&(flag, _)| attrs & flag != 0)
}

/// Returns `true` if the single-letter attribute flag `attr` is set. The
/// lookup is case-sensitive to match the distinct upper-case letters used
/// in the flag table.
pub fn has_attr(attrs: u32, attr: &str) -> bool {
    let attr = attr.trim();
    if attr.chars().count() != 1 {
        return false;
    }
    let ch = attr.chars().next().unwrap();
    for &(flag, letter) in FLAG_LETTERS {
        if letter == ch {
            return attrs & flag != 0;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_attrs_empty() {
        assert_eq!(format_attrs(0), "");
    }

    #[test]
    fn test_format_attrs_readonly() {
        assert_eq!(format_attrs(FILE_ATTRIBUTE_READONLY), "R");
    }

    #[test]
    fn test_format_attrs_multiple_in_table_order() {
        let attrs = FILE_ATTRIBUTE_ARCHIVE | FILE_ATTRIBUTE_READONLY | FILE_ATTRIBUTE_HIDDEN;
        // Order follows FLAG_LETTERS, not the order of the bits passed in.
        assert_eq!(format_attrs(attrs), "RHA");
    }

    #[test]
    fn test_has_any_attr() {
        assert!(!has_any_attr(0));
        assert!(has_any_attr(FILE_ATTRIBUTE_COMPRESSED));
    }

    #[test]
    fn test_has_attr() {
        let attrs = FILE_ATTRIBUTE_READONLY | FILE_ATTRIBUTE_COMPRESSED;
        assert!(has_attr(attrs, "R"));
        assert!(has_attr(attrs, "C"));
        assert!(has_attr(attrs, " C ")); // trimmed
        assert!(!has_attr(attrs, "H"));
    }

    #[test]
    fn test_has_attr_invalid() {
        assert!(!has_attr(FILE_ATTRIBUTE_READONLY, ""));
        assert!(!has_attr(FILE_ATTRIBUTE_READONLY, "RA"));
        assert!(!has_attr(FILE_ATTRIBUTE_READONLY, "Z"));
        // Letters are case-sensitive.
        assert!(!has_attr(FILE_ATTRIBUTE_READONLY, "r"));
    }
}
