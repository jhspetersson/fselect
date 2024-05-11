use crate::util::dimensions::DimensionsExtractor;
use crate::util::Dimensions;
use std::fs::File;
use std::io;
use std::io::Read;
use std::path::Path;

pub struct Mp4DimensionsExtractor;

impl DimensionsExtractor for Mp4DimensionsExtractor {
    fn supports_ext(&self, ext_lowercase: &str) -> bool {
        "mp4" == ext_lowercase
    }

    fn try_read_dimensions(&self, path: &Path) -> io::Result<Option<Dimensions>> {
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
                track.tkhd.as_ref().map(|tkhd| Dimensions {
                    width: (tkhd.width / 65536) as usize,
                    height: (tkhd.height / 65536) as usize,
                })
            }))
    }
}

#[cfg(test)]
mod test {
    use super::Mp4DimensionsExtractor;
    use crate::util::dimensions::{test::test_successful, Dimensions};
    use std::error::Error;

    #[test]
    fn test_success() -> Result<(), Box<dyn Error>> {
        test_successful(
            Mp4DimensionsExtractor,
            "video/rust-logo-blk.mp4",
            Some(Dimensions {
                width: 144,
                height: 144,
            }),
        )
    }
}
