/// Windows ACL detection and formatting via the Win32 Security API.
///
/// Uses `GetNamedSecurityInfoW` to retrieve the DACL, then inspects
/// its ACEs. A file "has ACL" if its DACL contains at least one
/// explicit (non-inherited) Access Control Entry.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{
    GetNamedSecurityInfoW, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    ACE_HEADER, ACL as WIN_ACL,
    DACL_SECURITY_INFORMATION, INHERITED_ACE,
    LookupAccountSidW, SID_NAME_USE,
};

const ACE_TYPE_ACCESS_ALLOWED: u8 = 0;
const ACE_TYPE_ACCESS_DENIED: u8 = 1;

/// Common access mask constants for NTFS files.
const FILE_ALL_ACCESS: u32 = 0x1F01FF;
const FILE_MODIFY: u32 = 0x1301BF;
const FILE_READ_EXECUTE: u32 = 0x1200A9;
const FILE_READ: u32 = 0x120089;
const FILE_WRITE: u32 = 0x120116;

/// Retrieve the DACL for a path. Returns the security descriptor pointer
/// (which must be freed with `LocalFree`) and the DACL pointer.
/// On failure returns `None`.
fn get_dacl(path: &Path) -> Option<(*mut u8, *const WIN_ACL)> {
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
            ptr::null_mut(),
            ptr::null_mut(),
            &mut p_dacl,
            ptr::null_mut(),
            &mut p_sd as *mut *mut u8 as *mut *mut core::ffi::c_void,
        )
    };

    if result != 0 {
        return None;
    }

    if p_dacl.is_null() {
        if !p_sd.is_null() {
            unsafe { LocalFree(p_sd as *mut core::ffi::c_void) };
        }
        return None;
    }

    Some((p_sd, p_dacl))
}

/// Returns `true` if the file at `path` has a DACL with at least one
/// explicit (non-inherited) ACE.
pub fn has_explicit_acl(path: &Path) -> bool {
    let Some((p_sd, p_dacl)) = get_dacl(path) else {
        return false;
    };

    let result = has_explicit_ace(p_dacl);

    unsafe { LocalFree(p_sd as *mut core::ffi::c_void) };
    result
}

/// Returns all explicit (non-inherited) ACEs as a formatted string.
/// Format: `allow:DOMAIN\User:full,deny:Guest:read,...`
/// Returns `None` if the DACL cannot be read or has no explicit ACEs.
pub fn format_acl(path: &Path) -> Option<String> {
    let Some((p_sd, p_dacl)) = get_dacl(path) else {
        return None;
    };

    let result = format_explicit_aces(p_dacl);

    unsafe { LocalFree(p_sd as *mut core::ffi::c_void) };

    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

/// Walk the ACEs in a DACL and return true if any are non-inherited.
fn has_explicit_ace(dacl: *const WIN_ACL) -> bool {
    let acl = unsafe { &*dacl };
    let ace_count = acl.AceCount as usize;
    if ace_count == 0 {
        return false;
    }

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

/// Walk the ACEs in a DACL and format all explicit (non-inherited) entries.
fn format_explicit_aces(dacl: *const WIN_ACL) -> String {
    let acl = unsafe { &*dacl };
    let ace_count = acl.AceCount as usize;
    if ace_count == 0 {
        return String::new();
    }

    let mut entries = Vec::new();
    let mut ace_ptr = unsafe { (dacl as *const u8).add(size_of::<WIN_ACL>()) };

    for _ in 0..ace_count {
        let header = unsafe { &*(ace_ptr as *const ACE_HEADER) };

        if header.AceFlags & (INHERITED_ACE as u8) == 0 {
            if let Some(entry) = format_ace(ace_ptr, header.AceType) {
                entries.push(entry);
            }
        }

        ace_ptr = unsafe { ace_ptr.add(header.AceSize as usize) };
    }

    entries.join(",")
}

/// Format a single ACE. The ACE layout for ACCESS_ALLOWED_ACE and
/// ACCESS_DENIED_ACE is identical: header (4 bytes), mask (4 bytes),
/// then the SID starting at offset 8.
fn format_ace(ace_ptr: *const u8, ace_type: u8) -> Option<String> {
    let type_str = match ace_type {
        ACE_TYPE_ACCESS_ALLOWED => "allow",
        ACE_TYPE_ACCESS_DENIED => "deny",
        _ => return None,
    };

    // Access mask is at offset 4 (right after ACE_HEADER).
    let mask = unsafe { *(ace_ptr.add(4) as *const u32) };

    // SID starts at offset 8.
    let sid_ptr = unsafe { ace_ptr.add(8) };

    let trustee = lookup_sid(sid_ptr as *mut core::ffi::c_void);
    let permissions = format_access_mask(mask);

    Some(format!("{}:{}:{}", type_str, trustee, permissions))
}

/// Resolve a SID to a `DOMAIN\Account` string. Falls back to a
/// raw `S-x-x-...` string if resolution fails.
fn lookup_sid(sid: *mut core::ffi::c_void) -> String {
    let mut name_buf = [0u16; 256];
    let mut domain_buf = [0u16; 256];
    let mut name_len: u32 = name_buf.len() as u32;
    let mut domain_len: u32 = domain_buf.len() as u32;
    let mut sid_use: SID_NAME_USE = 0;

    let ok = unsafe {
        LookupAccountSidW(
            ptr::null(),
            sid,
            name_buf.as_mut_ptr(),
            &mut name_len,
            domain_buf.as_mut_ptr(),
            &mut domain_len,
            &mut sid_use,
        )
    };

    if ok != 0 {
        let domain = String::from_utf16_lossy(&domain_buf[..domain_len as usize]);
        let name = String::from_utf16_lossy(&name_buf[..name_len as usize]);
        if domain.is_empty() {
            name
        } else {
            format!(r"{}\{}", domain, name)
        }
    } else {
        format_sid_fallback(sid)
    }
}

/// Format a SID as `S-1-...` string when LookupAccountSid fails.
fn format_sid_fallback(sid: *mut core::ffi::c_void) -> String {
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;

    let mut str_sid: *mut u16 = ptr::null_mut();
    let ok = unsafe { ConvertSidToStringSidW(sid, &mut str_sid) };

    if ok != 0 && !str_sid.is_null() {
        let len = unsafe {
            let mut p = str_sid;
            while *p != 0 {
                p = p.add(1);
            }
            p.offset_from(str_sid) as usize
        };
        let result = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(str_sid, len) });
        unsafe { LocalFree(str_sid as *mut core::ffi::c_void) };
        result
    } else {
        String::from("unknown")
    }
}

/// Map an access mask to a human-readable permission string.
fn format_access_mask(mask: u32) -> &'static str {
    match mask {
        FILE_ALL_ACCESS => "full",
        FILE_MODIFY => "modify",
        FILE_READ_EXECUTE => "rx",
        FILE_READ => "read",
        FILE_WRITE => "write",
        _ => mask_to_static(mask),
    }
}

/// For non-standard masks, return a hex string.
/// We use a small set of known combined masks plus a generic hex fallback.
fn mask_to_static(mask: u32) -> &'static str {
    // Leak a small string for uncommon masks. These are very few distinct
    // values in practice (typically <10 per system), so this is acceptable.
    let s = format!("0x{:08x}", mask);
    Box::leak(s.into_boxed_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_has_explicit_acl_on_temp() {
        let tmp = env::temp_dir();
        let _ = has_explicit_acl(&tmp);
    }

    #[test]
    fn test_has_explicit_acl_nonexistent() {
        let result = has_explicit_acl(Path::new(r"C:\nonexistent_file_fselect_test_12345"));
        assert!(!result);
    }

    #[test]
    fn test_format_acl_on_temp() {
        let tmp = env::temp_dir();
        // Should not panic; result depends on system config
        let _ = format_acl(&tmp);
    }

    #[test]
    fn test_format_acl_nonexistent() {
        let result = format_acl(Path::new(r"C:\nonexistent_file_fselect_test_12345"));
        assert!(result.is_none());
    }

    #[test]
    fn test_format_access_mask_known() {
        assert_eq!(format_access_mask(FILE_ALL_ACCESS), "full");
        assert_eq!(format_access_mask(FILE_MODIFY), "modify");
        assert_eq!(format_access_mask(FILE_READ_EXECUTE), "rx");
        assert_eq!(format_access_mask(FILE_READ), "read");
        assert_eq!(format_access_mask(FILE_WRITE), "write");
    }

    #[test]
    fn test_format_access_mask_unknown() {
        let result = format_access_mask(0x12345678);
        assert_eq!(result, "0x12345678");
    }
}
