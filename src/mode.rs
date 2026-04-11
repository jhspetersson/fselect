//! This module contains functions for working with file modes / permissions

use std::fs::Metadata;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;

pub fn get_mode(meta: &Metadata) -> String {
    #[cfg(unix)]
    {
        format_mode(meta.mode())
    }

    #[cfg(windows)]
    {
        format_mode(meta.file_attributes())
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = meta;
        String::new()
    }
}

pub fn format_mode(mode: u32) -> String {
    #[cfg(unix)]
    {
        get_mode_unix(mode)
    }

    #[cfg(windows)]
    {
        get_mode_windows(mode)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = mode;
        String::new()
    }
}

/// Format a Unix mode value regardless of platform (for archive entries).
pub fn format_unix_mode(mode: u32) -> String {
    let mut s = String::new();

    if mode & S_IFMT == 0o120000 {
        s.push('l')
    } else if mode & S_IFMT == S_IFBLK {
        s.push('b')
    } else if mode & S_IFMT == S_IFCHR {
        s.push('c')
    } else if mode & S_IFMT == S_IFSOCK {
        s.push('s')
    } else if mode & S_IFMT == S_IFIFO {
        s.push('p')
    } else if mode & S_IFMT == 0o040000 {
        s.push('d')
    } else {
        s.push('-')
    }

    s.push(if mode_user_read(mode) { 'r' } else { '-' });
    s.push(if mode_user_write(mode) { 'w' } else { '-' });
    s.push(if mode_user_exec(mode) {
        if mode_suid(mode) { 's' } else { 'x' }
    } else if mode_suid(mode) { 'S' } else { '-' });

    s.push(if mode_group_read(mode) { 'r' } else { '-' });
    s.push(if mode_group_write(mode) { 'w' } else { '-' });
    s.push(if mode_group_exec(mode) {
        if mode_sgid(mode) { 's' } else { 'x' }
    } else if mode_sgid(mode) { 'S' } else { '-' });

    s.push(if mode_other_read(mode) { 'r' } else { '-' });
    s.push(if mode_other_write(mode) { 'w' } else { '-' });
    s.push(if mode_other_exec(mode) {
        if mode_sticky(mode) { 't' } else { 'x' }
    } else if mode_sticky(mode) { 'T' } else { '-' });

    s
}

#[cfg(unix)]
fn get_mode_unix(mode: u32) -> String {
    let mut s = String::new();

    if mode_is_link(mode) {
        s.push('l')
    } else if mode_is_block_device(mode) {
        s.push('b')
    } else if mode_is_char_device(mode) {
        s.push('c')
    } else if mode_is_socket(mode) {
        s.push('s')
    } else if mode_is_pipe(mode) {
        s.push('p')
    } else if mode_is_directory(mode) {
        s.push('d')
    } else {
        s.push('-')
    }

    // user

    if mode_user_read(mode) {
        s.push('r')
    } else {
        s.push('-')
    }

    if mode_user_write(mode) {
        s.push('w')
    } else {
        s.push('-')
    }

    if mode_user_exec(mode) {
        if mode_suid(mode) {
            s.push('s')
        } else {
            s.push('x')
        }
    } else if mode_suid(mode) {
        s.push('S')
    } else {
        s.push('-')
    }

    // group

    if mode_group_read(mode) {
        s.push('r')
    } else {
        s.push('-')
    }

    if mode_group_write(mode) {
        s.push('w')
    } else {
        s.push('-')
    }

    if mode_group_exec(mode) {
        if mode_sgid(mode) {
            s.push('s')
        } else {
            s.push('x')
        }
    } else if mode_sgid(mode) {
        s.push('S')
    } else {
        s.push('-')
    }

    // other

    if mode_other_read(mode) {
        s.push('r')
    } else {
        s.push('-')
    }

    if mode_other_write(mode) {
        s.push('w')
    } else {
        s.push('-')
    }

    if mode_other_exec(mode) {
        if mode_sticky(mode) {
            s.push('t')
        } else {
            s.push('x')
        }
    } else if mode_sticky(mode) {
        s.push('T')
    } else {
        s.push('-')
    }

    s
}

#[allow(unused)]
pub fn get_mode_from_boxed_unix_int(meta: &Metadata) -> Option<u32> {
    #[cfg(unix)]
    {
        Some(meta.mode())
    }

    #[cfg(not(unix))]
    {
        None
    }
}

/// Generate a pair of functions: one that checks a mode bit on raw `u32`,
/// and one that extracts the mode from `Metadata` first.
macro_rules! mode_check {
    ($meta_fn:ident, $mode_fn:ident, $mask:expr, $expected:expr) => {
        pub fn $meta_fn(meta: &Metadata) -> bool {
            match get_mode_from_boxed_unix_int(meta) {
                Some(mode) => $mode_fn(mode),
                None => false,
            }
        }

        pub fn $mode_fn(mode: u32) -> bool {
            mode & $mask == $expected
        }
    };
}

mode_check!(user_read, mode_user_read, S_IRUSR, S_IRUSR);
mode_check!(user_write, mode_user_write, S_IWUSR, S_IWUSR);
mode_check!(user_exec, mode_user_exec, S_IXUSR, S_IXUSR);
mode_check!(group_read, mode_group_read, S_IRGRP, S_IRGRP);
mode_check!(group_write, mode_group_write, S_IWGRP, S_IWGRP);
mode_check!(group_exec, mode_group_exec, S_IXGRP, S_IXGRP);
mode_check!(other_read, mode_other_read, S_IROTH, S_IROTH);
mode_check!(other_write, mode_other_write, S_IWOTH, S_IWOTH);
mode_check!(other_exec, mode_other_exec, S_IXOTH, S_IXOTH);
mode_check!(suid_bit_set, mode_suid, S_ISUID, S_ISUID);
mode_check!(sgid_bit_set, mode_sgid, S_ISGID, S_ISGID);
mode_check!(sticky_bit_set, mode_sticky, S_ISVTX, S_ISVTX);
mode_check!(is_pipe, mode_is_pipe, S_IFMT, S_IFIFO);
mode_check!(is_char_device, mode_is_char_device, S_IFMT, S_IFCHR);
mode_check!(is_block_device, mode_is_block_device, S_IFMT, S_IFBLK);
mode_check!(is_socket, mode_is_socket, S_IFMT, S_IFSOCK);

pub fn user_all(meta: &Metadata) -> bool {
    user_read(meta) && user_write(meta) && user_exec(meta)
}

pub fn mode_user_all(mode: u32) -> bool {
    mode_user_read(mode) && mode_user_write(mode) && mode_user_exec(mode)
}

pub fn group_all(meta: &Metadata) -> bool {
    group_read(meta) && group_write(meta) && group_exec(meta)
}

pub fn mode_group_all(mode: u32) -> bool {
    mode_group_read(mode) && mode_group_write(mode) && mode_group_exec(mode)
}

pub fn other_all(meta: &Metadata) -> bool {
    other_read(meta) && other_write(meta) && other_exec(meta)
}

pub fn mode_other_all(mode: u32) -> bool {
    mode_other_read(mode) && mode_other_write(mode) && mode_other_exec(mode)
}

#[cfg(unix)]
pub fn mode_is_directory(mode: u32) -> bool {
    mode & S_IFMT == S_IFDIR
}

#[cfg(unix)]
pub fn mode_is_link(mode: u32) -> bool {
    mode & S_IFMT == S_IFLNK
}

const S_IRUSR: u32 = 0o400;
const S_IWUSR: u32 = 0o200;
const S_IXUSR: u32 = 0o100;

const S_IRGRP: u32 = 0o40;
const S_IWGRP: u32 = 0o20;
const S_IXGRP: u32 = 0o10;

const S_IROTH: u32 = 0o4;
const S_IWOTH: u32 = 0o2;
const S_IXOTH: u32 = 0o1;

const S_ISUID: u32 = 0o4000;
const S_ISGID: u32 = 0o2000;
const S_ISVTX: u32 = 0o1000;

const S_IFMT: u32 = 0o170000;
const S_IFBLK: u32 = 0o60000;
#[cfg(unix)]
const S_IFDIR: u32 = 0o40000;
const S_IFCHR: u32 = 0o20000;
const S_IFIFO: u32 = 0o10000;
#[cfg(unix)]
const S_IFLNK: u32 = 0o120000;
const S_IFSOCK: u32 = 0o140000;

#[cfg(windows)]
fn get_mode_windows(mode: u32) -> String {
    const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x20;
    const FILE_ATTRIBUTE_COMPRESSED: u32 = 0x800;
    const FILE_ATTRIBUTE_DEVICE: u32 = 0x40;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    const FILE_ATTRIBUTE_ENCRYPTED: u32 = 0x4000;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    const FILE_ATTRIBUTE_INTEGRITY_STREAM: u32 = 0x8000;
    const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
    const FILE_ATTRIBUTE_NOT_CONTENT_INDEXED: u32 = 0x2000;
    const FILE_ATTRIBUTE_NO_SCRUB_DATA: u32 = 0x20000;
    const FILE_ATTRIBUTE_OFFLINE: u32 = 0x1000;
    const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
    const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x400000;
    const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x40000;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    const FILE_ATTRIBUTE_SPARSE_FILE: u32 = 0x200;
    const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
    const FILE_ATTRIBUTE_TEMPORARY: u32 = 0x100;
    const FILE_ATTRIBUTE_VIRTUAL: u32 = 0x10000;

    let mut v = vec![];

    if mode & FILE_ATTRIBUTE_ARCHIVE == FILE_ATTRIBUTE_ARCHIVE {
        v.push("Archive");
    }

    if mode & FILE_ATTRIBUTE_COMPRESSED == FILE_ATTRIBUTE_COMPRESSED {
        v.push("Compressed");
    }

    if mode & FILE_ATTRIBUTE_DEVICE == FILE_ATTRIBUTE_DEVICE {
        v.push("Device");
    }

    if mode & FILE_ATTRIBUTE_DIRECTORY == FILE_ATTRIBUTE_DIRECTORY {
        v.push("Directory");
    }

    if mode & FILE_ATTRIBUTE_ENCRYPTED == FILE_ATTRIBUTE_ENCRYPTED {
        v.push("Encrypted");
    }

    if mode & FILE_ATTRIBUTE_HIDDEN == FILE_ATTRIBUTE_HIDDEN {
        v.push("Hidden");
    }

    if mode & FILE_ATTRIBUTE_INTEGRITY_STREAM == FILE_ATTRIBUTE_INTEGRITY_STREAM {
        v.push("Integrity Stream");
    }

    if mode & FILE_ATTRIBUTE_NORMAL == FILE_ATTRIBUTE_NORMAL {
        v.push("Normal");
    }

    if mode & FILE_ATTRIBUTE_NOT_CONTENT_INDEXED == FILE_ATTRIBUTE_NOT_CONTENT_INDEXED {
        v.push("Not indexed");
    }

    if mode & FILE_ATTRIBUTE_NO_SCRUB_DATA == FILE_ATTRIBUTE_NO_SCRUB_DATA {
        v.push("No scrub data");
    }

    if mode & FILE_ATTRIBUTE_OFFLINE == FILE_ATTRIBUTE_OFFLINE {
        v.push("Offline");
    }

    if mode & FILE_ATTRIBUTE_READONLY == FILE_ATTRIBUTE_READONLY {
        v.push("Readonly");
    }

    if mode & FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS == FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS {
        v.push("Recall on data access");
    }

    if mode & FILE_ATTRIBUTE_RECALL_ON_OPEN == FILE_ATTRIBUTE_RECALL_ON_OPEN {
        v.push("Recall on open");
    }

    if mode & FILE_ATTRIBUTE_REPARSE_POINT == FILE_ATTRIBUTE_REPARSE_POINT {
        v.push("Reparse point");
    }

    if mode & FILE_ATTRIBUTE_SPARSE_FILE == FILE_ATTRIBUTE_SPARSE_FILE {
        v.push("Sparse");
    }

    if mode & FILE_ATTRIBUTE_SYSTEM == FILE_ATTRIBUTE_SYSTEM {
        v.push("System");
    }

    if mode & FILE_ATTRIBUTE_TEMPORARY == FILE_ATTRIBUTE_TEMPORARY {
        v.push("Temporary");
    }

    if mode & FILE_ATTRIBUTE_VIRTUAL == FILE_ATTRIBUTE_VIRTUAL {
        v.push("Virtual");
    }

    v.join(", ")
}

#[allow(unused)]
pub fn get_uid(meta: &Metadata) -> Option<u32> {
    #[cfg(unix)]
    {
        Some(meta.uid())
    }

    #[cfg(not(unix))]
    {
        None
    }
}

#[allow(unused)]
pub fn get_gid(meta: &Metadata) -> Option<u32> {
    #[cfg(unix)]
    {
        Some(meta.gid())
    }

    #[cfg(not(unix))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_mode() {
        #[cfg(unix)]
        {
            // Regular file with rwxr-xr-- permissions (0754 in octal)
            let mode = 0o100754;
            assert_eq!(format_mode(mode), "-rwxr-xr--");

            // Directory with rwxr-xr-x permissions (0755 in octal)
            let mode = 0o40755;
            assert_eq!(format_mode(mode), "drwxr-xr-x");

            // Symbolic link with rwxrwxrwx permissions (0777 in octal)
            let mode = 0o120777;
            assert_eq!(format_mode(mode), "lrwxrwxrwx");

            // File with setuid bit (4755 in octal)
            let mode = 0o104755;
            assert_eq!(format_mode(mode), "-rwsr-xr-x");

            // File with setgid bit (2755 in octal)
            let mode = 0o102755;
            assert_eq!(format_mode(mode), "-rwxr-sr-x");

            // Directory with sticky bit (1755 in octal)
            let mode = 0o41755;
            assert_eq!(format_mode(mode), "drwxr-xr-t");
        }

        #[cfg(windows)]
        {
            const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
            const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
            const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;

            let mode = FILE_ATTRIBUTE_READONLY;
            assert_eq!(format_mode(mode), "Readonly");

            let mode = FILE_ATTRIBUTE_HIDDEN;
            assert_eq!(format_mode(mode), "Hidden");

            let mode = FILE_ATTRIBUTE_DIRECTORY;
            assert_eq!(format_mode(mode), "Directory");

            let mode = FILE_ATTRIBUTE_READONLY | FILE_ATTRIBUTE_HIDDEN;
            assert_eq!(format_mode(mode), "Hidden, Readonly");
        }
    }

    #[test]
    fn test_format_unix_mode() {
        assert_eq!(format_unix_mode(0o100644), "-rw-r--r--");
        assert_eq!(format_unix_mode(0o100755), "-rwxr-xr-x");
        assert_eq!(format_unix_mode(0o40755), "drwxr-xr-x");
        assert_eq!(format_unix_mode(0o120777), "lrwxrwxrwx");
        assert_eq!(format_unix_mode(0o104755), "-rwsr-xr-x");
    }

    #[test]
    fn test_mode_user_permissions() {
        let mode = 0o754; // rwxr-xr--

        assert!(mode_user_read(mode));
        assert!(mode_user_write(mode));
        assert!(mode_user_exec(mode));
        assert!(mode_user_all(mode));

        let mode = 0o654; // rw-r-xr--

        assert!(mode_user_read(mode));
        assert!(mode_user_write(mode));
        assert!(!mode_user_exec(mode));
        assert!(!mode_user_all(mode));
    }

    #[test]
    fn test_mode_group_permissions() {
        // Test group permission checks
        let mode = 0o754; // rwxr-xr--

        assert!(mode_group_read(mode));
        assert!(!mode_group_write(mode));
        assert!(mode_group_exec(mode));
        assert!(!mode_group_all(mode));

        let mode = 0o774; // rwxrwxr--

        assert!(mode_group_read(mode));
        assert!(mode_group_write(mode));
        assert!(mode_group_exec(mode));
        assert!(mode_group_all(mode));
    }

    #[test]
    fn test_mode_other_permissions() {
        let mode = 0o754; // rwxr-xr--

        assert!(mode_other_read(mode));
        assert!(!mode_other_write(mode));
        assert!(!mode_other_exec(mode));
        assert!(!mode_other_all(mode));

        let mode = 0o757; // rwxr-xrwx

        assert!(mode_other_read(mode));
        assert!(mode_other_write(mode));
        assert!(mode_other_exec(mode));
        assert!(mode_other_all(mode));
    }

    #[test]
    fn test_mode_special_bits() {
        // Test setuid bit
        let mode = 0o4755; // rwsr-xr-x
        assert!(mode_suid(mode));

        // Test setgid bit
        let mode = 0o2755; // rwxr-sr-x
        assert!(mode_sgid(mode));

        // Test sticky bit (Unix only)
        #[cfg(unix)]
        {
            let mode = 0o1755; // rwxr-xr-t
            assert!(mode_sticky(mode));
        }
    }

    #[test]
    fn test_mode_file_types() {
        // Test directory
        #[cfg(unix)]
        {
            let mode = 0o40755; // drwxr-xr-x
            assert!(mode_is_directory(mode));
            assert!(!mode_is_link(mode));
        }

        // Test symbolic link
        #[cfg(unix)]
        {
            let mode = 0o120755; // lrwxr-xr-x
            assert!(mode_is_link(mode));
            assert!(!mode_is_directory(mode));
        }

        // Test block device
        let mode = 0o60644; // brw-r--r--
        assert!(mode_is_block_device(mode));

        // Test character device
        let mode = 0o20644; // crw-r--r--
        assert!(mode_is_char_device(mode));

        // Test FIFO/pipe
        let mode = 0o10644; // prw-r--r--
        assert!(mode_is_pipe(mode));

        // Test socket
        let mode = 0o140644; // srw-r--r--
        assert!(mode_is_socket(mode));
    }

    #[test]
    fn test_block_device_not_char_device() {
        let mode = 0o60644;
        assert!(mode_is_block_device(mode));
        assert!(
            !mode_is_char_device(mode),
            "block device should not be detected as char device"
        );
    }

    #[test]
    fn test_symlink_not_char_device() {
        #[cfg(unix)]
        {
            let mode = 0o120777;
            assert!(mode_is_link(mode));
            assert!(
                !mode_is_char_device(mode),
                "symlink should not be detected as char device"
            );
        }
    }

    #[test]
    fn test_socket_not_directory() {
        #[cfg(unix)]
        {
            let mode = 0o140755;
            assert!(mode_is_socket(mode));
            assert!(
                !mode_is_directory(mode),
                "socket should not be detected as directory"
            );
        }
    }

    #[test]
    fn test_block_device_not_directory() {
        #[cfg(unix)]
        {
            let mode = 0o60755;
            assert!(mode_is_block_device(mode));
            assert!(
                !mode_is_directory(mode),
                "block device should not be detected as directory"
            );
        }
    }

    #[test]
    fn test_get_uid_gid() {
        // These functions are platform-specific, so we test the behavior
        // rather than the actual values

        #[cfg(unix)]
        {
            // On Unix, we should get Some value
            use std::fs::File;
            if let Ok(meta) = File::open("Cargo.toml").and_then(|f| f.metadata()) {
                assert!(get_uid(&meta).is_some());
                assert!(get_gid(&meta).is_some());
            }
        }

        #[cfg(not(unix))]
        {
            // On non-Unix platforms, we should get None
            use std::fs::File;
            if let Ok(meta) = File::open("Cargo.toml").and_then(|f| f.metadata()) {
                assert!(get_uid(&meta).is_none());
                assert!(get_gid(&meta).is_none());
            }
        }
    }
}
