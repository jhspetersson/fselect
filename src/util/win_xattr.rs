/// Windows extended attribute detection via NTFS Alternate Data Streams.
///
/// On Windows, the closest equivalent to Unix extended attributes are
/// NTFS Alternate Data Streams (ADS). This module uses `FindFirstStreamW`
/// / `FindNextStreamW` to enumerate them.

use std::path::Path;

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
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
        if stream.eq_ignore_ascii_case(&target) {
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

/// Reads the content of a named alternate data stream as a UTF-8 string.
/// Returns `None` if the stream does not exist or cannot be read as UTF-8.
pub fn read_named_ads(path: &Path, name: &str) -> Option<String> {
    use std::fs::File;
    use std::io::Read;

    let stream_path = format!("{}:{}", path.display(), name);
    let mut file = File::open(&stream_path).ok()?;
    let mut contents = String::new();
    file.read_to_string(&mut contents).ok()?;
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
}
