//! Windows extended attribute detection via NTFS Alternate Data Streams.
//!
//! On Windows, the closest equivalent to Unix extended attributes are
//! NTFS Alternate Data Streams (ADS). This module uses `FindFirstStreamW`
//! / `FindNextStreamW` to enumerate them.

use std::path::Path;

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Globalization::{CompareStringOrdinal, CSTR_EQUAL};
use windows_sys::Win32::Storage::FileSystem::{
    FindClose, FindFirstStreamW, FindNextStreamW, FindStreamInfoStandard, WIN32_FIND_STREAM_DATA,
};

use std::os::windows::ffi::OsStrExt;

/// Returns `true` if the file at `path` has any non-default alternate data streams.
pub fn has_any_ads(path: &Path) -> bool {
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut stream_data: WIN32_FIND_STREAM_DATA = unsafe { std::mem::zeroed() };

    let handle: HANDLE = unsafe {
        FindFirstStreamW(
            wide.as_ptr(),
            FindStreamInfoStandard,
            &mut stream_data as *mut _ as *mut core::ffi::c_void,
            0,
        )
    };

    if handle == INVALID_HANDLE_VALUE {
        return false;
    }

    // The first stream is typically the default data stream "::$DATA".
    // Any additional stream indicates an ADS (extended attribute equivalent).
    loop {
        let name = stream_name_from_data(&stream_data);
        if name != "::$DATA" {
            unsafe { FindClose(handle) };
            return true;
        }

        let ok = unsafe {
            FindNextStreamW(
                handle,
                &mut stream_data as *mut _ as *mut core::ffi::c_void,
            )
        };

        if ok == 0 {
            break;
        }
    }

    unsafe { FindClose(handle) };
    false
}

/// Returns the number of non-default alternate data streams on the file at `path`.
pub fn count_ads(path: &Path) -> usize {
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut stream_data: WIN32_FIND_STREAM_DATA = unsafe { std::mem::zeroed() };

    let handle: HANDLE = unsafe {
        FindFirstStreamW(
            wide.as_ptr(),
            FindStreamInfoStandard,
            &mut stream_data as *mut _ as *mut core::ffi::c_void,
            0,
        )
    };

    if handle == INVALID_HANDLE_VALUE {
        return 0;
    }

    let mut count = 0;

    loop {
        let name = stream_name_from_data(&stream_data);
        if name != "::$DATA" {
            count += 1;
        }

        let ok = unsafe {
            FindNextStreamW(
                handle,
                &mut stream_data as *mut _ as *mut core::ffi::c_void,
            )
        };

        if ok == 0 {
            break;
        }
    }

    unsafe { FindClose(handle) };
    count
}

/// Returns `true` if the file at `path` has an alternate data stream with the
/// given `name`. The name should be provided without the `:` prefix or `:$DATA`
/// suffix (e.g., just `"Zone.Identifier"`).
pub fn has_named_ads(path: &Path, name: &str) -> bool {
    let target = format!(":{}:$DATA", name);

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut stream_data: WIN32_FIND_STREAM_DATA = unsafe { std::mem::zeroed() };

    let handle: HANDLE = unsafe {
        FindFirstStreamW(
            wide.as_ptr(),
            FindStreamInfoStandard,
            &mut stream_data as *mut _ as *mut core::ffi::c_void,
            0,
        )
    };

    if handle == INVALID_HANDLE_VALUE {
        return false;
    }

    loop {
        let stream = stream_name_from_data(&stream_data);
        if eq_ignore_case(&stream, &target) {
            unsafe { FindClose(handle) };
            return true;
        }

        let ok = unsafe {
            FindNextStreamW(
                handle,
                &mut stream_data as *mut _ as *mut core::ffi::c_void,
            )
        };

        if ok == 0 {
            break;
        }
    }

    unsafe { FindClose(handle) };
    false
}

fn eq_ignore_case(a: &str, b: &str) -> bool {
    let a: Vec<u16> = a.encode_utf16().collect();
    let b: Vec<u16> = b.encode_utf16().collect();
    unsafe { CompareStringOrdinal(a.as_ptr(), a.len() as i32, b.as_ptr(), b.len() as i32, 1) == CSTR_EQUAL }
}

/// Reads the content of a named alternate data stream as a UTF-8 string.
/// Returns `None` if the stream does not exist or cannot be read as UTF-8.
pub fn read_named_ads(path: &Path, name: &str) -> Option<Vec<u8>> {
    use std::fs::File;
    use std::io::Read;

    let stream_path = format!("{}:{}", path.display(), name);
    let mut file = File::open(&stream_path).ok()?;
    let mut contents = Vec::new();
    file.read_to_end(&mut contents).ok()?;
    Some(contents)
}

/// Extract the stream name from a `WIN32_FIND_STREAM_DATA` as a Rust `String`.
fn stream_name_from_data(data: &WIN32_FIND_STREAM_DATA) -> String {
    let len = data
        .cStreamName
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(data.cStreamName.len());
    String::from_utf16_lossy(&data.cStreamName[..len])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_has_any_ads_on_temp() {
        let tmp = env::temp_dir();
        // Should not panic
        let _ = has_any_ads(&tmp);
    }

    #[test]
    fn test_has_any_ads_nonexistent() {
        let result = has_any_ads(Path::new(r"C:\nonexistent_file_fselect_test_xattr_12345"));
        assert!(!result);
    }

    #[test]
    fn test_has_named_ads_nonexistent() {
        let result = has_named_ads(
            Path::new(r"C:\nonexistent_file_fselect_test_xattr_12345"),
            "Zone.Identifier",
        );
        assert!(!result);
    }
    #[test]
    fn named_ads_lookup_ignores_unicode_case() {
        use std::fs;
        use std::io::Write;

        let tmp = env::temp_dir().join("fselect_test_unicode_ads");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let file = tmp.join("tagged.txt");
        fs::write(&file, "x").unwrap();
        for stream in ["\u{e9}cole", "\u{444}\u{430}\u{439}\u{43b}", "stra\u{df}e", "\u{fb00}", "\u{131}"] {
            let mut ads = fs::File::create(format!("{}:{}", file.display(), stream)).unwrap();
            ads.write_all(b"v").unwrap();
        }

        let accented = has_named_ads(&file, "\u{c9}COLE");
        let cyrillic = has_named_ads(&file, "\u{424}\u{410}\u{419}\u{41b}");
        let exact = has_named_ads(&file, "\u{e9}cole");
        let missing = has_named_ads(&file, "\u{e9}col");
        let sharp_s = has_named_ads(&file, "STRASSE");
        let ligature = has_named_ads(&file, "FF");
        let dotless = has_named_ads(&file, "I");
        let _ = fs::remove_dir_all(&tmp);

        assert!(exact, "exact");
        assert!(accented, "accented");
        assert!(cyrillic, "cyrillic");
        assert!(!missing, "missing");
        assert!(!sharp_s, "full case mapping of sharp s");
        assert!(!ligature, "full case mapping of a ligature");
        assert!(!dotless, "dotless i is not folded by NTFS");
    }

}
