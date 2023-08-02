use crate::util::duration::DurationExtractor;
use std::fs::File;
use std::io;
use crate::util::Duration;
use std::io::Read;
use std::path::Path;
use mp3_metadata::MP3Metadata;

pub struct Mp4DurationExtractor;

impl DurationExtractor for Mp4DurationExtractor {
    fn supports_ext(&self, ext_lowercase: &str) -> bool {
        "mp4" == ext_lowercase
    }

    fn try_read_duration(&self, path: &Path, _: &Option<MP3Metadata>) -> io::Result<Option<Duration>> {
        let mut fd = File::open(path)?;
        let mut buf = Vec::new();
        let _ = fd.read_to_end(&mut buf)?;
        let mut c = io::Cursor::new(&buf);
        let context = mp4parse::read_mp4(&mut c)?;
        Ok(context.tracks.iter()
            .find(|track| track.track_type == mp4parse::TrackType::Video)
            .and_then(|ref track| {
                track.tkhd.as_ref().map(|tkhd| {
                    Duration {
                        length: tkhd.duration as usize
                    }
                })
            }))
    }
}