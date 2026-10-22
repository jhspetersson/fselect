use std::cell::RefCell;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::rc::Rc;

use zip::DateTime;

pub type SharedArchive = Rc<RefCell<zip::ZipArchive<File>>>;

#[derive(Clone)]
pub struct ArchiveMember {
    pub archive: SharedArchive,
    pub index: usize,
}

#[derive(Clone)]
pub struct FileInfo {
    pub name: String,
    pub size: u64,
    pub mode: Option<u32>,
    pub modified: Option<DateTime>,
    pub member: Option<ArchiveMember>,
}

impl FileInfo {
    pub fn is_dir(&self) -> bool {
        self.name.ends_with('/')
            || self.name.ends_with('\\')
            || self.mode.is_some_and(crate::mode::mode_is_directory)
    }

    #[cfg(test)]
    pub fn named(name: &str) -> FileInfo {
        FileInfo {
            name: name.to_string(),
            size: 0,
            mode: None,
            modified: None,
            member: None,
        }
    }
}

/// How many levels below the archive an entry sits: `sub/deep.txt` is two,
/// a directory entry `sub/` is one.
pub fn archived_entry_levels(name: &str) -> u32 {
    name.split('/').filter(|component| !component.is_empty()).count() as u32
}

#[cfg(test)]
thread_local! {
    static ARCHIVE_OPENS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub fn archive_opens() -> usize {
    ARCHIVE_OPENS.with(|opens| opens.get())
}

pub fn open_archive(path: &Path) -> Option<zip::ZipArchive<File>> {
    #[cfg(test)]
    ARCHIVE_OPENS.with(|opens| opens.set(opens.get() + 1));
    let file = File::open(path).ok()?;
    zip::ZipArchive::new(file).ok()
}

/// Hands the member's contents to `read`: through the archive the walk
/// already holds open when `file_info` carries one, otherwise by opening the
/// zip at `archive` and looking the member up by name.
/// `None` when the archive or the entry cannot be opened.
pub fn with_archived_entry<T>(archive: &Path, file_info: &FileInfo, read: impl FnOnce(&mut dyn Read) -> T) -> Option<T> {
    match &file_info.member {
        Some(member) => {
            let mut archive = member.archive.borrow_mut();
            let mut entry = archive.by_index(member.index).ok()?;
            Some(read(&mut entry))
        }
        None => {
            let mut archive = open_archive(archive)?;
            let index = archive_member_index(&archive, &file_info.name)?;
            let mut entry = archive.by_index(index).ok()?;
            Some(read(&mut entry))
        }
    }
}

pub fn archived_directory_has_children(archive: &Path, file_info: &FileInfo) -> Option<bool> {
    fn has_children<'a>(prefix: &str, names: impl Iterator<Item = &'a str>) -> bool {
        names
            .map(|name| name.replace('\\', "/"))
            .any(|name| name != prefix && name.starts_with(prefix))
    }

    let mut prefix = file_info.name.replace('\\', "/");
    if !prefix.ends_with('/') {
        prefix.push('/');
    }
    match &file_info.member {
        Some(member) => Some(has_children(&prefix, member.archive.borrow().file_names())),
        None => Some(has_children(&prefix, open_archive(archive)?.file_names())),
    }
}

pub fn archive_member_index<R>(archive: &zip::ZipArchive<R>, name: &str) -> Option<usize>
where
    R: std::io::Read + std::io::Seek
{
    let backslashed = name.replace('/', "\\");
    [name.to_string(), format!("{}/", name), backslashed.clone(), format!("{}\\", backslashed)]
        .iter()
        .find_map(|candidate| archive.index_for_name(candidate))
}

pub fn to_file_info<R>(zipped_file: &zip::read::ZipFile<R>, member: Option<ArchiveMember>) -> FileInfo
where
    R: std::io::Read + std::io::Seek
{
    FileInfo {
        name: zipped_file.name().replace('\\', "/"),
        size: zipped_file.size(),
        mode: zipped_file.unix_mode(),
        modified: zipped_file.last_modified(),
        member,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;

    use super::*;

    #[test]
    fn shared_archive_members_are_read_without_reopening_the_zip() {
        let tmp = std::env::temp_dir().join("fselect_test_fileinfo_shared_archive");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("pack.zip");
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("a.txt", options).unwrap();
        zip.write_all(b"first").unwrap();
        zip.start_file("b.txt", options).unwrap();
        zip.write_all(b"second").unwrap();
        zip.finish().unwrap();

        let archive = Rc::new(RefCell::new(open_archive(&path).unwrap()));
        let shared = to_file_info(
            &archive.borrow_mut().by_index(1).unwrap(),
            Some(ArchiveMember { archive: Rc::clone(&archive), index: 1 }),
        );
        let by_name = FileInfo::named("b.txt");
        let read_all = |reader: &mut dyn Read| {
            let mut text = String::new();
            reader.read_to_string(&mut text).unwrap();
            text
        };

        let before = archive_opens();
        let shared_text = with_archived_entry(&path, &shared, read_all);
        let shared_opens = archive_opens() - before;
        let by_name_text = with_archived_entry(&path, &by_name, read_all);
        let by_name_opens = archive_opens() - before - shared_opens;
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(shared.name, "b.txt");
        assert_eq!(shared_text.as_deref(), Some("second"));
        assert_eq!(shared_opens, 0);
        assert_eq!(by_name_text.as_deref(), Some("second"));
        assert_eq!(by_name_opens, 1);
    }
}
