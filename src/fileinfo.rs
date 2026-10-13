use std::fs::File;
use std::io::Read;
use std::path::Path;

use zip::DateTime;

pub struct FileInfo {
    pub name: String,
    pub size: u64,
    pub mode: Option<u32>,
    pub modified: Option<DateTime>,
}

/// How many levels below the archive an entry sits: `sub/deep.txt` is two,
/// a directory entry `sub/` is one.
pub fn archived_entry_levels(name: &str) -> u32 {
    name.split('/').filter(|component| !component.is_empty()).count() as u32
}

/// Opens `name` inside the zip at `archive` and hands its contents to `read`.
/// `None` when the archive or the entry cannot be opened.
pub fn with_archived_entry<T>(archive: &Path, name: &str, read: impl FnOnce(&mut dyn Read) -> T) -> Option<T> {
    let file = File::open(archive).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut entry = archive.by_name(name).ok()?;
    Some(read(&mut entry))
}

pub fn to_file_info<R>(zipped_file: &zip::read::ZipFile<R>) -> FileInfo
where
    R: std::io::Read + std::io::Seek
{
    FileInfo {
        name: zipped_file.name().to_string(),
        size: zipped_file.size(),
        mode: zipped_file.unix_mode(),
        modified: zipped_file.last_modified(),
    }
}
