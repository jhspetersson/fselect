use std::io;
use std::path::Path;

use crate::util::dimensions::DimensionsExtractor;
use crate::util::mp4::{is_mp4_ext, read_mp4_info};
use crate::util::Dimensions;

pub struct Mp4DimensionsExtractor;

impl DimensionsExtractor for Mp4DimensionsExtractor {
    fn supports_ext(&self, ext_lowercase: &str) -> bool {
        is_mp4_ext(ext_lowercase)
    }

    fn try_read_dimensions(&self, path: &Path) -> io::Result<Option<Dimensions>> {
        read_mp4_info(path).map(|info| info.dimensions)
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
