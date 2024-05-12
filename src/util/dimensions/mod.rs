use std::io;

mod image;
mod mkv;
mod mp4;
mod svg;

use self::svg::SvgDimensionsExtractor;
use image::ImageDimensionsExtractor;
use mkv::MkvDimensionsExtractor;
use mp4::Mp4DimensionsExtractor;
use std::path::Path;

#[derive(PartialEq, Eq, Clone, Debug)]
pub struct Dimensions {
    pub width: usize,
    pub height: usize,
}

pub trait DimensionsExtractor {
    fn supports_ext(&self, ext_lowercase: &str) -> bool;
    fn try_read_dimensions(&self, path: &Path) -> io::Result<Option<Dimensions>>;
}

const EXTRACTORS: [&dyn DimensionsExtractor; 4] = [
    &MkvDimensionsExtractor,
    &Mp4DimensionsExtractor,
    &SvgDimensionsExtractor,
    &ImageDimensionsExtractor,
];

pub fn get_dimensions<T: AsRef<Path>>(path: T) -> Option<Dimensions> {
    let path_ref = path.as_ref();
    let extension = path_ref.extension()?.to_str()?;

    EXTRACTORS
        .iter()
        .find(|extractor| extractor.supports_ext(&extension.to_lowercase()))
        .and_then(|extractor| extractor.try_read_dimensions(path_ref).unwrap_or_default())
}

#[cfg(test)]
mod test {
    use crate::util::dimensions::DimensionsExtractor;
    use crate::util::Dimensions;
    use std::error::Error;
    use std::ffi::OsStr;
    use std::path::PathBuf;

    pub(crate) fn test_successful<T: DimensionsExtractor>(
        under_test: T,
        test_res_path: &str,
        expected: Option<Dimensions>,
    ) -> Result<(), Box<dyn Error>> {
        let path_string = std::env::var("CARGO_MANIFEST_DIR")? + "/resources/test/" + test_res_path;
        let path = PathBuf::from(path_string);
        assert!(under_test.supports_ext(path.extension().and_then(OsStr::to_str).unwrap()));
        assert_eq!(under_test.try_read_dimensions(&path)?, expected);

        Ok(())
    }

    pub(crate) fn test_fail<T: DimensionsExtractor>(
        under_test: T,
        test_res_path: &str,
        expected: std::io::ErrorKind,
    ) -> Result<(), Box<dyn Error>> {
        let path_string = std::env::var("CARGO_MANIFEST_DIR")? + "/resources/test/" + test_res_path;
        let path = PathBuf::from(path_string);
        assert!(under_test.supports_ext(path.extension().and_then(OsStr::to_str).unwrap()));
        let result = under_test.try_read_dimensions(&path);
        assert_eq!(result.map_err(|err| err.kind()), Err(expected));

        Ok(())
    }
}
