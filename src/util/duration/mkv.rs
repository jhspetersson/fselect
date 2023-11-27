use mp3_metadata::MP3Metadata;
use std::fs::File;
use std::io;
use std::path::Path;

use crate::util::duration::DurationExtractor;
use crate::util::Duration;

use matroska::MatroskaError;

pub struct MkvDurationExtractor;

impl DurationExtractor for MkvDurationExtractor {
    fn supports_ext(&self, ext_lowercase: &str) -> bool {
        "mkv" == ext_lowercase || "webm" == ext_lowercase
    }

    fn try_read_duration(
        &self,
        path: &Path,
        _: &Option<MP3Metadata>,
    ) -> io::Result<Option<Duration>> {
        let fd = File::open(path)?;
        let matroska = matroska::Matroska::open(fd).map_err(|err| match err {
            MatroskaError::Io(io) => io,
            MatroskaError::UTF8(utf8) => io::Error::new(io::ErrorKind::InvalidData, utf8),
            e => io::Error::new(io::ErrorKind::InvalidData, e),
        })?;

        match matroska.info.duration {
            Some(duration) => {
                return Ok(Some(Duration {
                    length: duration.as_secs() as usize,
                }))
            }
            None => return Ok(None),
        }
    }
}

#[cfg(test)]
mod test {
    use super::MkvDurationExtractor;
    use crate::util::duration::DurationExtractor;
    use crate::util::Duration;
    use std::error::Error;
    use std::path::PathBuf;

    #[test]
    fn test_success() -> Result<(), Box<dyn Error>> {
        let path_string =
            std::env::var("CARGO_MANIFEST_DIR")? + "/resources/test/" + "video/rust-logo-blk.mkv";
        let path = PathBuf::from(path_string);
        assert_eq!(
            MkvDurationExtractor.try_read_duration(&path, &None)?,
            Some(Duration { length: 1 }),
        );
        Ok(())
    }
}
