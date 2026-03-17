/// Windows ACL detection via the Win32 Security API.
///
/// Uses `GetNamedSecurityInfoW` to retrieve the DACL, then inspects
/// its ACEs. A file "has ACL" if its DACL contains at least one
/// explicit (non-inherited) Access Control Entry.

use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{
    GetNamedSecurityInfoW, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    ACL as WIN_ACL, DACL_SECURITY_INFORMATION,
    ACE_HEADER, INHERITED_ACE,
};

/// Returns `true` if the file at `path` has a DACL with at least one
/// explicit (non-inherited) ACE.
pub fn has_explicit_acl(path: &Path) -> bool {
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut p_sd: *mut u8 = ptr::null_mut();
    let mut p_dacl: *mut WIN_ACL = ptr::null_mut();

    let result = unsafe {
        GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(), // owner sid
            ptr::null_mut(), // group sid
            &mut p_dacl,     // dacl
            ptr::null_mut(), // sacl
            &mut p_sd as *mut *mut u8 as *mut *mut core::ffi::c_void,
        )
    };

    if result != 0 {
        return false;
    }

    let has_acl = if p_dacl.is_null() {
        false
    } else {
        has_explicit_ace(p_dacl)
    };

    if !p_sd.is_null() {
        unsafe { LocalFree(p_sd as *mut core::ffi::c_void) };
    }

    has_acl
}

/// Walk the ACEs in a DACL and return true if any are non-inherited.
fn has_explicit_ace(dacl: *const WIN_ACL) -> bool {
    let acl = unsafe { &*dacl };
    let ace_count = acl.AceCount as usize;
    if ace_count == 0 {
        return false;
    }

    // The first ACE starts right after the ACL header (8 bytes).
    let mut ace_ptr = unsafe { (dacl as *const u8).add(size_of::<WIN_ACL>()) };

    for _ in 0..ace_count {
        let header = unsafe { &*(ace_ptr as *const ACE_HEADER) };
        if header.AceFlags & (INHERITED_ACE as u8) == 0 {
            return true;
        }
        ace_ptr = unsafe { ace_ptr.add(header.AceSize as usize) };
    }

    false
}

use std::os::windows::ffi::OsStrExt;

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_has_explicit_acl_on_temp() {
        // Temp files typically have explicit ACEs
        let tmp = env::temp_dir();
        // Should not panic, result depends on system config
        let _ = has_explicit_acl(&tmp);
    }

    #[test]
    fn test_has_explicit_acl_nonexistent() {
        let result = has_explicit_acl(Path::new(r"C:\nonexistent_file_fselect_test_12345"));
        assert!(!result);
    }
}
