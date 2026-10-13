use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, DirEntry};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct DirEntryLookup {
    directory: Option<PathBuf>,
    entries: HashMap<OsString, DirEntry>,
    directory_reads: usize,
}

impl DirEntryLookup {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn entry(&mut self, directory: &Path, file_name: &OsStr) -> io::Result<Option<&DirEntry>> {
        if self.directory.as_deref() != Some(directory) {
            self.directory_reads += 1;
            self.entries = fs::read_dir(directory)?
                .flatten()
                .map(|entry| (entry.file_name(), entry))
                .collect();
            self.directory = Some(directory.to_path_buf());
        }
        Ok(self.entries.get(file_name))
    }

    #[cfg(test)]
    pub fn directory_reads(&self) -> usize {
        self.directory_reads
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn make_dir(name: &str, files: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for file in files {
            fs::write(dir.join(file), file).unwrap();
        }
        dir
    }

    #[test]
    fn consecutive_paths_in_one_directory_share_a_single_read() {
        let names: Vec<String> = (0..50).map(|i| format!("file{:02}.txt", i)).collect();
        let dir = make_dir("fselect_test_dir_lookup_single", &names.iter().map(String::as_str).collect::<Vec<_>>());

        let mut lookup = DirEntryLookup::new();
        for name in &names {
            let entry = lookup.entry(&dir, OsStr::new(name)).unwrap().expect(name);
            assert_eq!(entry.file_name(), OsStr::new(name));
            assert_eq!(entry.path(), dir.join(name));
        }
        assert_eq!(lookup.directory_reads(), 1, "every lookup in the same directory must reuse the first read");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn changing_directory_reads_again_and_unknown_names_are_absent() {
        let first = make_dir("fselect_test_dir_lookup_first", &["a.txt"]);
        let second = make_dir("fselect_test_dir_lookup_second", &["b.txt"]);

        let mut lookup = DirEntryLookup::new();
        assert!(lookup.entry(&first, OsStr::new("a.txt")).unwrap().is_some());
        assert!(lookup.entry(&second, OsStr::new("b.txt")).unwrap().is_some());
        assert!(lookup.entry(&first, OsStr::new("a.txt")).unwrap().is_some());
        assert_eq!(lookup.directory_reads(), 3);

        assert!(lookup.entry(&first, OsStr::new("missing.txt")).unwrap().is_none());
        assert!(lookup.entry(&first, OsStr::new("b.txt")).unwrap().is_none());
        assert_eq!(lookup.directory_reads(), 3, "a miss in the cached directory must not re-read it");

        let gone = first.join("nope");
        assert!(lookup.entry(&gone, OsStr::new("x")).is_err());

        let _ = fs::remove_dir_all(&first);
        let _ = fs::remove_dir_all(&second);
    }
}
