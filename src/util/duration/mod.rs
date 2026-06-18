mod mkv;
mod mp4;

use std::io;
use std::path::Path;

use mkv::MkvDurationExtractor;
use mp4::Mp4DurationExtractor;

#[derive(PartialEq, Eq, Clone, Debug)]
pub struct Duration {
    pub length: usize,
}

/// Extracts the duration of a video container. Audio formats are handled
/// separately by [`crate::util::audio`] via `lofty`, so the remaining
/// extractors only cover the video containers `lofty` does not read (MP4,
/// Matroska).
pub trait DurationExtractor {
    fn supports_ext(&self, ext_lowercase: &str) -> bool;
    fn try_read_duration(&self, path: &Path) -> io::Result<Option<Duration>>;
}

const EXTRACTORS: [&dyn DurationExtractor; 2] = [
    &Mp4DurationExtractor,
    &MkvDurationExtractor,
];

pub fn get_duration<T: AsRef<Path>>(path: T) -> Option<Duration> {
    let path_ref = path.as_ref();
    let extension = path_ref.extension()?.to_str()?;

    EXTRACTORS
        .iter()
        .find(|extractor| extractor.supports_ext(&extension.to_lowercase()))
        .and_then(|extractor| {
            extractor
                .try_read_duration(path_ref)
                .unwrap_or_default()
        })
}
