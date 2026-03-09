use std::fs::File;
use std::os::unix::io::AsRawFd;

/// FS_IOC_GETFLAGS ioctl request number.
/// Computed as _IOR('f', 1, c_long) = (2 << 30) | (sizeof(c_long) << 16) | ('f' << 8) | 1
const fn fs_ioc_getflags() -> libc::c_ulong {
    let dir: libc::c_ulong = 2; // _IOC_READ
    let ty: libc::c_ulong = b'f' as libc::c_ulong;
    let nr: libc::c_ulong = 1;
    let size: libc::c_ulong = std::mem::size_of::<libc::c_long>() as libc::c_ulong;
    (dir << 30) | (size << 16) | (ty << 8) | nr
}

const FS_SECRM_FL: libc::c_long = 0x00000001;
const FS_UNRM_FL: libc::c_long = 0x00000002;
const FS_COMPR_FL: libc::c_long = 0x00000004;
const FS_SYNC_FL: libc::c_long = 0x00000008;
const FS_IMMUTABLE_FL: libc::c_long = 0x00000010;
const FS_APPEND_FL: libc::c_long = 0x00000020;
const FS_NODUMP_FL: libc::c_long = 0x00000040;
const FS_NOATIME_FL: libc::c_long = 0x00000080;
const FS_ENCRYPT_FL: libc::c_long = 0x00000800;
const FS_INDEX_FL: libc::c_long = 0x00001000;
const FS_JOURNAL_DATA_FL: libc::c_long = 0x00004000;
const FS_NOTAIL_FL: libc::c_long = 0x00008000;
const FS_DIRSYNC_FL: libc::c_long = 0x00010000;
const FS_TOPDIR_FL: libc::c_long = 0x00020000;
const FS_EXTENT_FL: libc::c_long = 0x00080000;
const FS_VERITY_FL: libc::c_long = 0x00100000;
const FS_NOCOW_FL: libc::c_long = 0x00800000;
const FS_DAX_FL: libc::c_long = 0x02000000;
const FS_INLINE_DATA_FL: libc::c_long = 0x10000000;
const FS_PROJINHERIT_FL: libc::c_long = 0x20000000;
const FS_CASEFOLD_FL: libc::c_long = 0x40000000;

const FLAG_LETTERS: &[(libc::c_long, char)] = &[
    (FS_SECRM_FL, 's'),
    (FS_UNRM_FL, 'u'),
    (FS_COMPR_FL, 'c'),
    (FS_SYNC_FL, 'S'),
    (FS_IMMUTABLE_FL, 'i'),
    (FS_APPEND_FL, 'a'),
    (FS_NODUMP_FL, 'd'),
    (FS_NOATIME_FL, 'A'),
    (FS_ENCRYPT_FL, 'E'),
    (FS_INDEX_FL, 'I'),
    (FS_JOURNAL_DATA_FL, 'j'),
    (FS_NOTAIL_FL, 't'),
    (FS_DIRSYNC_FL, 'D'),
    (FS_TOPDIR_FL, 'T'),
    (FS_EXTENT_FL, 'e'),
    (FS_VERITY_FL, 'V'),
    (FS_NOCOW_FL, 'C'),
    (FS_DAX_FL, 'x'),
    (FS_INLINE_DATA_FL, 'N'),
    (FS_PROJINHERIT_FL, 'P'),
    (FS_CASEFOLD_FL, 'F'),
];

pub fn get_ext_attrs(file: &File) -> Option<libc::c_long> {
    let fd = file.as_raw_fd();
    let mut flags: libc::c_long = 0;
    let ret = unsafe { libc::ioctl(fd, fs_ioc_getflags(), &mut flags) };
    if ret == 0 {
        Some(flags)
    } else {
        None
    }
}

pub fn format_ext_attrs(flags: libc::c_long) -> String {
    let mut result = String::new();
    for &(flag, letter) in FLAG_LETTERS {
        if flags & flag != 0 {
            result.push(letter);
        }
    }
    result
}

pub fn has_ext_attr(flags: libc::c_long, attr: &str) -> bool {
    let attr = attr.trim();
    if attr.len() != 1 {
        return false;
    }
    let ch = attr.chars().next().unwrap();
    for &(flag, letter) in FLAG_LETTERS {
        if letter == ch {
            return flags & flag != 0;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_ext_attrs_empty() {
        assert_eq!(format_ext_attrs(0), "");
    }

    #[test]
    fn test_format_ext_attrs_immutable() {
        assert_eq!(format_ext_attrs(FS_IMMUTABLE_FL), "i");
    }

    #[test]
    fn test_format_ext_attrs_multiple() {
        let flags = FS_IMMUTABLE_FL | FS_APPEND_FL | FS_EXTENT_FL;
        assert_eq!(format_ext_attrs(flags), "iae");
    }

    #[test]
    fn test_has_ext_attr() {
        let flags = FS_IMMUTABLE_FL | FS_EXTENT_FL;
        assert!(has_ext_attr(flags, "i"));
        assert!(has_ext_attr(flags, "e"));
        assert!(!has_ext_attr(flags, "a"));
    }

    #[test]
    fn test_has_ext_attr_invalid() {
        assert!(!has_ext_attr(0, ""));
        assert!(!has_ext_attr(0, "zz"));
        assert!(!has_ext_attr(0, "z"));
    }
}