use crate::util::duration::DurationExtractor;
use crate::util::Duration;
use mp3_metadata::MP3Metadata;
use std::io;
use std::path::Path;

pub struct Mp3DurationExtractor;

impl DurationExtractor for Mp3DurationExtractor {
    fn supports_ext(&self, ext_lowercase: &str) -> bool {
        "mp3" == ext_lowercase
    }

    fn try_read_duration(
        &self,
        _: &Path,
        mp3_metadata: &Option<MP3Metadata>,
    ) -> io::Result<Option<Duration>> {
        match mp3_metadata {
            Some(mp3_metadata) => Ok(Some(Duration {
                length: mp3_metadata.duration.as_secs() as usize,
            })),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod test {
    use crate::util::duration::DurationExtractor;
    use crate::util::duration::Mp3DurationExtractor;
    use crate::util::Duration;
    use crate::util::MP3Metadata;
    use crate::PathBuf;
    use std::error::Error;

    #[test]
    fn test_success() -> Result<(), Box<dyn Error>> {
        let path_string =
            std::env::var("CARGO_MANIFEST_DIR")? + "/resources/test/" + "audio/silent-35s.mp3";
        let path = PathBuf::from(path_string);

        let mp3_metadata = |path: PathBuf| -> Option<MP3Metadata> {
            match mp3_metadata::read_from_file(path) {
                Ok(mp3_meta) => Some(mp3_meta),
                _ => None,
            }
        }(path.clone());

        assert_eq!(
            Mp3DurationExtractor.try_read_duration(&path, &mp3_metadata)?,
            Some(Duration { length: 35 }),
        );
        Ok(())
    }
}
