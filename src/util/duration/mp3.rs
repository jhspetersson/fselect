use std::io;
use std::path::Path;
use mp3_metadata::MP3Metadata;
use crate::util::Duration;
use crate::util::duration::DurationExtractor;

pub struct Mp3DurationExtractor;

impl DurationExtractor for Mp3DurationExtractor {
    fn supports_ext(&self, ext_lowercase: &str) -> bool {
        "mp3" == ext_lowercase
    }

    fn try_read_duration(&self, _: &Path, mp3_metadata: &Option<MP3Metadata>) -> io::Result<Option<Duration>> {
        match mp3_metadata {
            Some(mp3_metadata) => Ok(Some(Duration { length: mp3_metadata.duration.as_secs() as usize })),
            None => Ok(None)
        }
    }
}