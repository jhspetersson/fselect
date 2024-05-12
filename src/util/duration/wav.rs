use std::io;
use std::path::Path;

use mp3_metadata::MP3Metadata;

use wavers::Wav;

use crate::util::duration::DurationExtractor;
use crate::util::Duration;

pub struct WavDurationExtractor;

impl DurationExtractor for WavDurationExtractor {
    fn supports_ext(&self, ext_lowercase: &str) -> bool {
        "wav" == ext_lowercase
    }

    fn try_read_duration(
        &self,
        path: &Path,
        _: &Option<MP3Metadata>,
    ) -> io::Result<Option<Duration>> {
        let wav: Wav<i16> =
            Wav::from_path(path).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        Ok(Some(Duration {
            length: wav.duration() as usize,
        }))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::util::duration::DurationExtractor;
    use crate::util::Duration;
    use std::error::Error;
    use std::path::PathBuf;

    #[test]
    fn test_success() -> Result<(), Box<dyn Error>> {
        let path_string =
            std::env::var("CARGO_MANIFEST_DIR")? + "/resources/test/" + "audio/silent.wav";
        let path = PathBuf::from(path_string);
        assert_eq!(
            WavDurationExtractor.try_read_duration(&path, &None)?,
            Some(Duration { length: 15 }),
        );
        Ok(())
    }
}
