use std::path::PathBuf;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::HMODULE;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

const EVERYTHING_REQUEST_FULL_PATH_AND_FILE_NAME: u32 = 0x0000_0004;

#[derive(Debug)]
pub enum EverythingError {
    Unavailable,
    Query(u32),
}

type FnSetSearchW = unsafe extern "system" fn(*const u16);
type FnSetRequestFlags = unsafe extern "system" fn(u32);
type FnSetMatchPath = unsafe extern "system" fn(i32);
type FnQueryW = unsafe extern "system" fn(i32) -> i32;
type FnGetNumResults = unsafe extern "system" fn() -> u32;
type FnGetResultFullPathNameW = unsafe extern "system" fn(u32, *mut u16, u32) -> u32;
type FnGetLastError = unsafe extern "system" fn() -> u32;
type FnAction = unsafe extern "system" fn();

struct Api {
    set_search: FnSetSearchW,
    set_request_flags: FnSetRequestFlags,
    set_match_path: FnSetMatchPath,
    query: FnQueryW,
    get_num_results: FnGetNumResults,
    get_full_path: FnGetResultFullPathNameW,
    get_last_error: FnGetLastError,
    reset: FnAction,
    cleanup: FnAction,
}

unsafe impl Send for Api {}
unsafe impl Sync for Api {}

static API: OnceLock<Option<Api>> = OnceLock::new();

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn load_api() -> Option<Api> {
    unsafe {
        let primary = if cfg!(target_pointer_width = "64") {
            "Everything64.dll"
        } else {
            "Everything32.dll"
        };

        let mut handle: HMODULE = LoadLibraryW(wide(primary).as_ptr());
        if handle.is_null() {
            handle = LoadLibraryW(wide("Everything.dll").as_ptr());
        }
        if handle.is_null() {
            return None;
        }

        macro_rules! proc {
            ($name:literal, $ty:ty) => {{
                let p = GetProcAddress(handle, $name.as_ptr())?;
                std::mem::transmute::<unsafe extern "system" fn() -> isize, $ty>(p)
            }};
        }

        Some(Api {
            set_search: proc!(b"Everything_SetSearchW\0", FnSetSearchW),
            set_request_flags: proc!(b"Everything_SetRequestFlags\0", FnSetRequestFlags),
            set_match_path: proc!(b"Everything_SetMatchPath\0", FnSetMatchPath),
            query: proc!(b"Everything_QueryW\0", FnQueryW),
            get_num_results: proc!(b"Everything_GetNumResults\0", FnGetNumResults),
            get_full_path: proc!(b"Everything_GetResultFullPathNameW\0", FnGetResultFullPathNameW),
            get_last_error: proc!(b"Everything_GetLastError\0", FnGetLastError),
            reset: proc!(b"Everything_Reset\0", FnAction),
            cleanup: proc!(b"Everything_CleanUp\0", FnAction),
        })
    }
}

pub fn query_descendants(root_abspath: &str) -> Result<Vec<PathBuf>, EverythingError> {
    let api = API
        .get_or_init(load_api)
        .as_ref()
        .ok_or(EverythingError::Unavailable)?;

    let mut prefix = root_abspath.to_string();
    if !prefix.ends_with('\\') {
        prefix.push('\\');
    }
    let search = wide(&format!("\"{}\"", prefix));

    unsafe {
        (api.reset)();
        (api.set_match_path)(1);
        (api.set_request_flags)(EVERYTHING_REQUEST_FULL_PATH_AND_FILE_NAME);
        (api.set_search)(search.as_ptr());

        if (api.query)(1) == 0 {
            return Err(EverythingError::Query((api.get_last_error)()));
        }

        let count = (api.get_num_results)();
        let mut results = Vec::with_capacity(count as usize);
        let mut buf = vec![0u16; 32768];
        for i in 0..count {
            let len = (api.get_full_path)(i, buf.as_mut_ptr(), buf.len() as u32);
            if len > 0 {
                results.push(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])));
            }
        }

        (api.cleanup)();
        Ok(results)
    }
}
