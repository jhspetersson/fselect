use zip::DateTime;

pub struct FileInfo {
    pub name: String,
    pub size: u64,
    pub mode: Option<u32>,
    pub modified: Option<DateTime>,
}

pub fn to_file_info(zipped_file: &zip::read::ZipFile) -> FileInfo {
    FileInfo {
        name: zipped_file.name().to_string(),
        size: zipped_file.size(),
        mode: zipped_file.unix_mode(),
        modified: zipped_file.last_modified(),
    }
}
