use std::collections::HashMap;
use std::fs::{DirEntry, FileType, Metadata};
use std::io::Read;
use std::path::Path;

#[cfg(all(unix, feature = "users"))]
use uzers::UsersCache;

use crate::config::Config;
use crate::fileinfo::{FileInfo, with_archived_entry};
use crate::util::*;
#[cfg(feature = "git")]
use crate::util::git::GitCache;
use crate::util::audio::{AudioInfo, get_audio_info};
use crate::util::dimensions::get_dimensions;
use crate::util::duration::get_duration;
use crate::util::mp4::{Mp4Info, get_mp4_info};

const MIME_SAMPLE_SIZE: u64 = 64 * 1024;

fn archived_mime_type(archive: &Path, file_info: &FileInfo) -> Option<String> {
    if file_info.is_dir() {
        return None;
    }
    with_archived_entry(archive, file_info, |reader| {
        let mut sample = Vec::new();
        reader.take(MIME_SAMPLE_SIZE).read_to_end(&mut sample).ok()?;
        Some(String::from(tree_magic_mini::from_u8(&sample)))
    })
    .flatten()
}

pub struct FileMetadataState {
    pub(crate) file_metadata: Option<Option<Metadata>>,
    pub(crate) entry_file_type: Option<Option<FileType>>,
    pub(crate) content_stats: Option<Option<ContentStats>>,
    pub(crate) content_kind: Option<Option<ContentKind>>,
    full_content_scan_expected: bool,
    pub(crate) dimensions: Option<Option<Dimensions>>,
    pub(crate) duration: Option<Option<Duration>>,
    pub(crate) audio_info: Option<Option<AudioInfo>>,
    pub(crate) exif_metadata: Option<Option<HashMap<String, String>>>,
    pub(crate) mime_type: Option<Option<String>>,
    pub(crate) mp4_info: Option<Option<Mp4Info>>,
    pub(crate) hashes: [Option<String>; HashAlgorithm::COUNT],
    planned_hashes: Vec<HashAlgorithm>,
}

impl FileMetadataState {
    pub fn new() -> FileMetadataState {
        FileMetadataState {
            file_metadata: None,
            entry_file_type: None,
            content_stats: None,
            content_kind: None,
            full_content_scan_expected: false,
            dimensions: None,
            duration: None,
            audio_info: None,
            exif_metadata: None,
            mime_type: None,
            mp4_info: None,
            hashes: [const { None }; HashAlgorithm::COUNT],
            planned_hashes: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        let full_content_scan_expected = self.full_content_scan_expected;
        let planned_hashes = std::mem::take(&mut self.planned_hashes);
        *self = Self::new();
        self.full_content_scan_expected = full_content_scan_expected;
        self.planned_hashes = planned_hashes;
    }

    pub fn plan_hashes(&mut self, algorithms: Vec<HashAlgorithm>) {
        self.planned_hashes = algorithms;
    }

    pub fn expect_full_content_scan(&mut self, expected: bool) {
        self.full_content_scan_expected = expected;
    }

    pub fn update_file_metadata(&mut self, entry: &DirEntry, follow_symlinks: bool) {
        if self.file_metadata.is_none() {
            self.file_metadata = Some(get_metadata(entry, follow_symlinks));
        }
    }

    pub fn get_file_metadata(&self) -> Option<&Metadata> {
        self.file_metadata.as_ref().and_then(|o| o.as_ref())
    }

    pub fn get_file_metadata_as_option(&self) -> &Option<Metadata> {
        static NONE: Option<Metadata> = None;
        self.file_metadata.as_ref().unwrap_or(&NONE)
    }

    /// Whether a metadata load has already been attempted for the current file
    /// (regardless of whether it succeeded). Lets type predicates reuse it
    /// instead of issuing a fresh stat.
    pub fn file_metadata_loaded(&self) -> bool {
        self.file_metadata.is_some()
    }

    /// Seed the entry's file type from a value the caller already resolved
    /// (e.g. the directory traversal's descent check), so type predicates can
    /// reuse it. A `None` hint is ignored, leaving the slot to be filled lazily
    /// on first use.
    pub fn seed_file_type(&mut self, file_type: Option<FileType>) {
        if file_type.is_some() {
            self.entry_file_type = Some(file_type);
        }
    }

    /// The entry's file type, computed once and memoised. Reflects the entry
    /// itself (symlinks are not followed), so it answers is_symlink directly
    /// and is_dir/is_file only when not following symlinks.
    pub fn get_or_compute_file_type(&mut self, entry: &DirEntry) -> Option<FileType> {
        if self.entry_file_type.is_none() {
            self.entry_file_type = Some(entry.file_type().ok());
        }
        self.entry_file_type.flatten()
    }

    pub fn update_line_count(&mut self, entry: &DirEntry, file_info: &Option<FileInfo>) {
        // Always derived from the content-stats pass: a raw newline-byte scan
        // disagrees with the decoded text for UTF-16/32 files (0x0A occurs
        // inside multibyte code units), which made the reported count depend
        // on which content field happened to be evaluated first.
        self.update_content_stats(entry, file_info);
    }

    pub fn get_line_count(&self) -> Option<usize> {
        self.get_content_stats().map(|stats| stats.line_count)
    }

    pub fn update_content_stats(&mut self, entry: &DirEntry, file_info: &Option<FileInfo>) {
        if self.content_stats.is_none() {
            self.content_stats = Some(match file_info {
                Some(file_info) => {
                    with_archived_entry(&entry.path(), file_info, |reader| content_stats_from_reader(reader)).flatten()
                }
                None => get_content_stats(entry),
            });
        }
    }

    pub fn get_content_stats(&self) -> Option<&ContentStats> {
        self.content_stats.as_ref().and_then(|o| o.as_ref())
    }

    pub fn update_content_kind(&mut self, entry: &DirEntry, file_info: &Option<FileInfo>) {
        if self.content_kind.is_some() {
            return;
        }
        if self.full_content_scan_expected {
            self.update_content_stats(entry, file_info);
        }
        if let Some(stats) = &self.content_stats {
            self.content_kind = Some(stats.as_ref().map(ContentKind::from));
            return;
        }
        self.content_kind = Some(match file_info {
            Some(file_info) => {
                with_archived_entry(&entry.path(), file_info, |reader| content_kind_from_reader(reader)).flatten()
            }
            None => get_content_kind(entry),
        });
    }

    pub fn get_content_kind(&self) -> Option<&ContentKind> {
        self.content_kind.as_ref().and_then(|o| o.as_ref())
    }

    pub fn update_audio_info(&mut self, entry: &DirEntry) {
        if self.audio_info.is_none() {
            self.audio_info = Some(get_audio_info(&entry.path()));
        }
    }

    pub fn get_audio_info(&self) -> Option<&AudioInfo> {
        self.audio_info.as_ref().and_then(|o| o.as_ref())
    }

    pub fn update_exif_metadata(&mut self, entry: &DirEntry) {
        if self.exif_metadata.is_none() {
            self.exif_metadata = Some(get_exif_metadata(entry));
        }
    }

    pub fn get_exif_metadata(&self) -> Option<&HashMap<String, String>> {
        self.exif_metadata.as_ref().and_then(|o| o.as_ref())
    }

    pub fn get_exif_string(&mut self, entry: &DirEntry, key: &str) -> Option<Variant> {
        self.update_exif_metadata(entry);
        self.get_exif_metadata()
            .and_then(|info| info.get(key))
            .map(Variant::from_string)
    }

    pub fn update_mime_type(&mut self, entry: &DirEntry, file_info: &Option<FileInfo>) {
        if self.mime_type.is_none() {
            self.mime_type = Some(match file_info {
                Some(file_info) => archived_mime_type(&entry.path(), file_info),
                None => tree_magic_mini::from_filepath(&entry.path()).map(String::from),
            });
        }
    }

    pub fn get_mime_type(&self) -> Option<&str> {
        self.mime_type.as_ref().and_then(|o| o.as_deref())
    }

    pub fn get_or_compute_hash(&mut self, entry: &DirEntry, algorithm: HashAlgorithm) -> &str {
        if self.hashes[algorithm as usize].is_none() {
            let mut batch = vec![algorithm];
            if self.planned_hashes.contains(&algorithm) {
                batch.extend(
                    self.planned_hashes
                        .iter()
                        .copied()
                        .filter(|planned| *planned != algorithm && self.hashes[*planned as usize].is_none()),
                );
            }
            for (computed, hash) in batch.iter().zip(get_file_hashes(entry, &batch)) {
                self.hashes[*computed as usize] = Some(hash);
            }
        }
        self.hashes[algorithm as usize].as_deref().unwrap()
    }

    fn update_mp4_info(&mut self, entry: &DirEntry) -> Option<&Mp4Info> {
        self.mp4_info.get_or_insert_with(|| get_mp4_info(&entry.path())).as_ref()
    }

    pub fn update_dimensions(&mut self, entry: &DirEntry) {
        if self.dimensions.is_none() {
            let dimensions = match self.update_mp4_info(entry) {
                Some(info) => info.dimensions.clone(),
                None => get_dimensions(entry.path()),
            };
            self.dimensions = Some(dimensions);
        }
    }

    pub fn get_dimensions(&self) -> Option<&Dimensions> {
        self.dimensions.as_ref().and_then(|o| o.as_ref())
    }

    pub fn update_duration(&mut self, entry: &DirEntry) {
        if self.duration.is_none() {
            // Audio durations come from lofty (via the cached audio info);
            // anything it doesn't handle falls back to the video extractors.
            self.update_audio_info(entry);
            let mut duration = self
                .get_audio_info()
                .and_then(|info| info.duration)
                .map(|length| Duration { length });
            if duration.is_none() {
                duration = match self.update_mp4_info(entry) {
                    Some(info) => info.duration.clone(),
                    None => get_duration(entry.path()),
                };
            }
            self.duration = Some(duration);
        }
    }

    pub fn get_duration(&self) -> Option<&Duration> {
        self.duration.as_ref().and_then(|o| o.as_ref())
    }
}

pub struct FieldContext<'a> {
    pub entry: &'a DirEntry,
    pub file_info: &'a Option<FileInfo>,
    pub root_path: &'a Path,
    pub fms: &'a mut FileMetadataState,
    #[cfg(feature = "git")]
    pub git_cache: &'a mut GitCache,
    pub follow_symlinks: bool,
    pub config: &'a Config,
    pub default_config: &'a Config,
    #[cfg(all(unix, feature = "users"))]
    pub user_cache: &'a UsersCache,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_metadata_state_new() {
        let state = FileMetadataState::new();

        assert!(state.file_metadata.is_none());
        assert!(state.entry_file_type.is_none());
        assert!(state.content_stats.is_none());
        assert!(state.dimensions.is_none());
        assert!(state.duration.is_none());
        assert!(state.audio_info.is_none());
        assert!(state.exif_metadata.is_none());
        assert!(state.mime_type.is_none());
        assert!(state.mp4_info.is_none());
        assert!(state.hashes.iter().all(Option::is_none));
    }

    #[test]
    fn test_file_metadata_state_clear() {
        let mut state = FileMetadataState::new();

        state.file_metadata = Some(None);
        state.entry_file_type = Some(None);
        state.content_stats = Some(None);
        state.dimensions = Some(None);
        state.duration = Some(None);
        state.audio_info = Some(None);
        state.exif_metadata = Some(None);
        state.mime_type = Some(None);
        state.mp4_info = Some(None);
        state.hashes = [const { Some(String::new()) }; HashAlgorithm::COUNT];
        state.plan_hashes(vec![HashAlgorithm::Sha1, HashAlgorithm::Sha3]);

        state.clear();

        assert!(state.file_metadata.is_none());
        assert!(state.entry_file_type.is_none());
        assert!(state.content_stats.is_none());
        assert!(state.dimensions.is_none());
        assert!(state.duration.is_none());
        assert!(state.audio_info.is_none());
        assert!(state.exif_metadata.is_none());
        assert!(state.mime_type.is_none());
        assert!(state.mp4_info.is_none());
        assert!(state.hashes.iter().all(Option::is_none));
        assert_eq!(state.planned_hashes, vec![HashAlgorithm::Sha1, HashAlgorithm::Sha3]);
    }

    fn single_entry(dir_name: &str, file_name: &str, content: &[u8]) -> (std::path::PathBuf, DirEntry) {
        let dir = std::env::temp_dir().join(dir_name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(file_name), content).unwrap();
        let entry = std::fs::read_dir(&dir).unwrap().next().unwrap().unwrap();
        (dir, entry)
    }

    #[test]
    fn planned_hashes_are_computed_together_and_others_on_demand() {
        let (dir, entry) = single_entry("fselect_test_planned_hashes", "abc.txt", b"abc");
        let mut state = FileMetadataState::new();
        state.plan_hashes(vec![HashAlgorithm::Sha1, HashAlgorithm::Sha256]);

        assert_eq!(state.get_or_compute_hash(&entry, HashAlgorithm::Sha512).len(), 128);
        assert!(state.hashes[HashAlgorithm::Sha1 as usize].is_none());
        assert!(state.hashes[HashAlgorithm::Sha256 as usize].is_none());

        assert_eq!(
            state.get_or_compute_hash(&entry, HashAlgorithm::Sha1),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            state.hashes[HashAlgorithm::Sha256 as usize].as_deref(),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
        assert!(state.hashes[HashAlgorithm::Sha3 as usize].is_none());

        std::fs::remove_file(entry.path()).unwrap();
        assert_eq!(
            state.get_or_compute_hash(&entry, HashAlgorithm::Sha256),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(state.get_or_compute_hash(&entry, HashAlgorithm::Sha3), "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mp4_dimensions_and_duration_share_one_parse() {
        let source = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/test/video/rust-logo-blk.mp4");
        let (dir, entry) = single_entry("fselect_test_shared_mp4_parse", "clip.mp4", &std::fs::read(source).unwrap());
        let mut state = FileMetadataState::new();

        state.update_dimensions(&entry);
        assert_eq!(state.get_dimensions(), Some(&Dimensions { width: 144, height: 144 }));

        std::fs::remove_file(entry.path()).unwrap();
        state.update_duration(&entry);
        assert_eq!(state.get_duration(), Some(&Duration { length: 1 }));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
