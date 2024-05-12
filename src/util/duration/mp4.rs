use crate::util::duration::DurationExtractor;
use crate::util::Duration;
use mp3_metadata::MP3Metadata;
use std::fs::File;
use std::io;
use std::io::Read;
use std::path::Path;

pub struct Mp4DurationExtractor;

impl DurationExtractor for Mp4DurationExtractor {
    fn supports_ext(&self, ext_lowercase: &str) -> bool {
        "mp4" == ext_lowercase
    }

    fn try_read_duration(
        &self,
        path: &Path,
        _: &Option<MP3Metadata>,
    ) -> io::Result<Option<Duration>> {
        let mut fd = File::open(path)?;
        let mut buf = Vec::new();
        let _ = fd.read_to_end(&mut buf)?;
        let mut c = io::Cursor::new(&buf);
        let context = mp4parse::read_mp4(&mut c)?;
        Ok(context
            .tracks
            .iter()
            .find(|track| track.track_type == mp4parse::TrackType::Video)
            .and_then(|ref track| {
                track.tkhd.as_ref().map(|tkhd| Duration {
                    length: (tkhd.duration / 1000) as usize,
                })
            }))
    }
}

#[cfg(test)]
mod test {
    use super::Mp4DurationExtractor;
    use crate::util::duration::DurationExtractor;
    use crate::util::Duration;
    use crate::PathBuf;
    use std::error::Error;

    #[test]
    fn test_success() -> Result<(), Box<dyn Error>> {
        let path_string =
            std::env::var("CARGO_MANIFEST_DIR")? + "/resources/test/" + "video/rust-logo-blk.mp4";
        let path = PathBuf::from(path_string);
        assert_eq!(
            Mp4DurationExtractor.try_read_duration(&path, &None)?,
            Some(Duration { length: 1 }),
        );
        Ok(())
    }
}
