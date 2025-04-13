use zip::DateTime;

pub struct FileInfo {
    pub name: String,
    pub size: u64,
    pub mode: Option<u32>,
    pub modified: Option<DateTime>,
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
