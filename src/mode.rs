use std::fs::Metadata;

pub fn print_mode(meta: &Box<Metadata>) {
    let mode: u32;

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        mode = meta.mode();
        print_mode_unix(mode);
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        mode = meta.file_attributes();
        print_mode_windows(mode);
    }
}

fn print_mode_unix(mode: u32) {
    let mut s = String::new();

    // user

    if mode & S_IRUSR == mode {
        s.push('r')
    } else {
        s.push('-');
    }

    if mode & S_IWUSR == mode {
        s.push('w')
    } else {
        s.push('-');
    }

    if mode & S_IXUSR == mode {
        s.push('x')
    } else {
        s.push('-');
    }

    // group

    if mode & S_IRGRP == mode {
        s.push('r')
    } else {
        s.push('-');
    }

    if mode & S_IWGRP == mode {
        s.push('w')
    } else {
        s.push('-');
    }

    if mode & S_IXGRP == mode {
        s.push('x')
    } else {
        s.push('-');
    }

    // other

    if mode & S_IROTH == mode {
        s.push('r')
    } else {
        s.push('-');
    }

    if mode & S_IWOTH == mode {
        s.push('w')
    } else {
        s.push('-');
    }

    if mode & S_IXOTH == mode {
        s.push('x')
    } else {
        s.push('-');
    }

    println!("{}", s);
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

fn print_mode_windows(mode: u32) {
    let mut v = vec![];

    if mode & FILE_ATTRIBUTE_ARCHIVE == mode {
        v.push("Archive");
    }

    if mode & FILE_ATTRIBUTE_COMPRESSED == mode {
        v.push("Compressed");
    }

    if mode & FILE_ATTRIBUTE_DEVICE == mode {
        v.push("Device");
    }

    if mode & FILE_ATTRIBUTE_DIRECTORY == mode {
        v.push("Directory");
    }

    if mode & FILE_ATTRIBUTE_ENCRYPTED == mode {
        v.push("Encrypted");
    }

    if mode & FILE_ATTRIBUTE_HIDDEN == mode {
        v.push("Hidden");
    }

    if mode & FILE_ATTRIBUTE_INTEGRITY_STREAM == mode {
        v.push("Integrity Stream");
    }

    if mode & FILE_ATTRIBUTE_NORMAL == mode {
        v.push("Normal");
    }

    if mode & FILE_ATTRIBUTE_NOT_CONTENT_INDEXED == mode {
        v.push("Not indexed");
    }

    if mode & FILE_ATTRIBUTE_NO_SCRUB_DATA == mode {
        v.push("No scrub data");
    }

    if mode & FILE_ATTRIBUTE_OFFLINE == mode {
        v.push("Offline");
    }

    if mode & FILE_ATTRIBUTE_READONLY == mode {
        v.push("Readonly");
    }

    if mode & FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS == mode {
        v.push("Recall on data access");
    }

    if mode & FILE_ATTRIBUTE_RECALL_ON_OPEN == mode {
        v.push("Recall on open");
    }

    if mode & FILE_ATTRIBUTE_REPARSE_POINT == mode {
        v.push("Reparse point");
    }

    if mode & FILE_ATTRIBUTE_SPARSE_FILE == mode {
        v.push("Sparse");
    }

    if mode & FILE_ATTRIBUTE_SYSTEM == mode {
        v.push("System");
    }

    if mode & FILE_ATTRIBUTE_TEMPORARY == mode {
        v.push("Temporary");
    }

    if mode & FILE_ATTRIBUTE_VIRTUAL == mode {
        v.push("Virtual");
    }

    println!("{}", v.join(", "));
}

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