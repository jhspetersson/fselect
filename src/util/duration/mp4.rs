use std::io;
use std::path::Path;

use crate::util::duration::DurationExtractor;
use crate::util::mp4::{is_mp4_ext, read_mp4_info};
use crate::util::Duration;

pub struct Mp4DurationExtractor;

impl DurationExtractor for Mp4DurationExtractor {
    fn supports_ext(&self, ext_lowercase: &str) -> bool {
        is_mp4_ext(ext_lowercase)
    }

    fn try_read_duration(&self, path: &Path) -> io::Result<Option<Duration>> {
        read_mp4_info(path).map(|info| info.duration)
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
            Mp4DurationExtractor.try_read_duration(&path)?,
            Some(Duration { length: 1 }),
        );
        Ok(())
    }

    #[test]
    fn audio_only_mp4_reports_its_duration() -> Result<(), Box<dyn Error>> {
        let path_string =
            std::env::var("CARGO_MANIFEST_DIR")? + "/resources/test/" + "audio/tone-5s.mp4";
        let path = PathBuf::from(path_string);
        assert_eq!(
            Mp4DurationExtractor.try_read_duration(&path)?,
            Some(Duration { length: 5 }),
        );
        Ok(())
    }
}
