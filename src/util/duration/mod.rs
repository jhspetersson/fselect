use std::io;

mod mkv;
mod mp3;
mod mp4;
mod wav;

use std::path::Path;

use mp3_metadata::MP3Metadata;

use mkv::MkvDurationExtractor;
use mp3::Mp3DurationExtractor;
use mp4::Mp4DurationExtractor;
use wav::WavDurationExtractor;

#[derive(PartialEq, Eq, Clone, Debug)]
pub struct Duration {
    pub length: usize,
}

pub trait DurationExtractor {
    fn supports_ext(&self, ext_lowercase: &str) -> bool;
    fn try_read_duration(
        &self,
        path: &Path,
        mp3_metadata: &Option<MP3Metadata>,
    ) -> io::Result<Option<Duration>>;
}

const EXTRACTORS: [&dyn DurationExtractor; 4] = [
    &Mp3DurationExtractor,
    &Mp4DurationExtractor,
    &MkvDurationExtractor,
    &WavDurationExtractor,
];

pub fn get_duration<T: AsRef<Path>>(
    path: T,
    mp3_metadata: &Option<MP3Metadata>,
) -> Option<Duration> {
    let path_ref = path.as_ref();
    let extension = path_ref.extension()?.to_str()?;

    EXTRACTORS
        .iter()
        .find(|extractor| extractor.supports_ext(&extension.to_lowercase()))
        .and_then(|extractor| {
            extractor
                .try_read_duration(path_ref, mp3_metadata)
                .unwrap_or_default()
        })
}
