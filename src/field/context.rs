use std::collections::HashMap;
use std::fs::{DirEntry, Metadata};
use std::path::Path;

use mp3_metadata::MP3Metadata;
#[cfg(all(unix, feature = "users"))]
use uzers::UsersCache;

use crate::config::Config;
use crate::fileinfo::FileInfo;
use crate::util::*;
use crate::util::dimensions::get_dimensions;
use crate::util::duration::get_duration;

pub struct FileMetadataState {
    pub(crate) file_metadata: Option<Option<Metadata>>,
    pub(crate) line_count: Option<Option<usize>>,
    pub(crate) dimensions: Option<Option<Dimensions>>,
    pub(crate) duration: Option<Option<Duration>>,
    pub(crate) mp3_metadata: Option<Option<MP3Metadata>>,
    pub(crate) exif_metadata: Option<Option<HashMap<String, String>>>,
    pub(crate) mime_type: Option<Option<String>>,
}

impl FileMetadataState {
    pub fn new() -> FileMetadataState {
        FileMetadataState {
            file_metadata: None,
            line_count: None,
            dimensions: None,
            duration: None,
            mp3_metadata: None,
            exif_metadata: None,
            mime_type: None,
        }
    }

    pub fn clear(&mut self) {
        *self = Self::new();
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

    pub fn update_line_count(&mut self, entry: &DirEntry) {
        if self.line_count.is_none() {
            self.line_count = Some(get_line_count(entry));
        }
    }

    pub fn get_line_count(&self) -> Option<usize> {
        self.line_count.flatten()
    }

    pub fn update_mp3_metadata(&mut self, entry: &DirEntry) {
        if self.mp3_metadata.is_none() {
            self.mp3_metadata = Some(get_mp3_metadata(entry));
        }
    }

    pub fn get_mp3_metadata(&self) -> Option<&MP3Metadata> {
        self.mp3_metadata.as_ref().and_then(|o| o.as_ref())
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
            .map(|v| Variant::from_string(v))
    }

    pub fn update_mime_type(&mut self, entry: &DirEntry) {
        if self.mime_type.is_none() {
            self.mime_type = Some(
                tree_magic_mini::from_filepath(&entry.path()).map(String::from)
            );
        }
    }

    pub fn get_mime_type(&self) -> Option<&str> {
        self.mime_type.as_ref().and_then(|o| o.as_deref())
    }

    pub fn update_dimensions(&mut self, entry: &DirEntry) {
        if self.dimensions.is_none() {
            self.dimensions = Some(get_dimensions(entry.path()));
        }
    }

    pub fn get_dimensions(&self) -> Option<&Dimensions> {
        self.dimensions.as_ref().and_then(|o| o.as_ref())
    }

    pub fn update_duration(&mut self, entry: &DirEntry) {
        if self.duration.is_none() {
            self.update_mp3_metadata(entry);
            let mp3_flat = self.mp3_metadata.as_ref().unwrap_or(&None);
            self.duration = Some(get_duration(entry.path(), mp3_flat));
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
        assert!(state.line_count.is_none());
        assert!(state.dimensions.is_none());
        assert!(state.duration.is_none());
        assert!(state.mp3_metadata.is_none());
        assert!(state.exif_metadata.is_none());
        assert!(state.mime_type.is_none());
    }

    #[test]
    fn test_file_metadata_state_clear() {
        let mut state = FileMetadataState::new();

        state.file_metadata = Some(None);
        state.line_count = Some(None);
        state.dimensions = Some(None);
        state.duration = Some(None);
        state.mp3_metadata = Some(None);
        state.exif_metadata = Some(None);
        state.mime_type = Some(None);

        state.clear();

        assert!(state.file_metadata.is_none());
        assert!(state.line_count.is_none());
        assert!(state.dimensions.is_none());
        assert!(state.duration.is_none());
        assert!(state.mp3_metadata.is_none());
        assert!(state.exif_metadata.is_none());
        assert!(state.mime_type.is_none());
    }
}
