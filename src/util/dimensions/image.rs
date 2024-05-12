use crate::util::dimensions::DimensionsExtractor;
use crate::util::Dimensions;
use imagesize::ImageError;
use std::io;
use std::path::Path;

pub struct ImageDimensionsExtractor;

impl ImageDimensionsExtractor {
    const EXTENSIONS: [&'static str; 13] = [
        "bmp", "gif", "heic", "heif", "jpeg", "jpg", "jxl", "png", "psb", "psd", "tga", "tiff",
        "webp",
    ];
}

impl DimensionsExtractor for ImageDimensionsExtractor {
    fn supports_ext(&self, ext_lowercase: &str) -> bool {
        ImageDimensionsExtractor::EXTENSIONS
            .iter()
            .any(|&supported| supported == ext_lowercase)
    }

    fn try_read_dimensions(&self, path: &Path) -> io::Result<Option<Dimensions>> {
        let dimensions = imagesize::size(path).map_err(|err| match err {
            ImageError::NotSupported => {
                io::Error::new(io::ErrorKind::InvalidInput, ImageError::NotSupported)
            }
            ImageError::CorruptedImage => {
                io::Error::new(io::ErrorKind::InvalidData, ImageError::CorruptedImage)
            }
            ImageError::IoError(e) => e,
        })?;
        Ok(Some(Dimensions {
            width: dimensions.width,
            height: dimensions.height,
        }))
    }
}

#[cfg(test)]
mod test {
    use super::ImageDimensionsExtractor;
    use crate::util::dimensions::{test::test_successful, Dimensions};
    use std::error::Error;

    fn do_test_success(ext: &str, w: usize, h: usize) -> Result<(), Box<dyn Error>> {
        let res_path = String::from("image/rust-logo-blk.") + ext;
        test_successful(
            ImageDimensionsExtractor,
            &res_path,
            Some(Dimensions {
                width: w,
                height: h,
            }),
        )
    }

    #[test]
    pub fn test_bmp() -> Result<(), Box<dyn Error>> {
        do_test_success("bmp", 144, 144)
    }

    #[test]
    pub fn test_gif() -> Result<(), Box<dyn Error>> {
        do_test_success("gif", 144, 144)
    }

    #[test]
    pub fn test_jpeg() -> Result<(), Box<dyn Error>> {
        do_test_success("jpeg", 144, 144)
    }

    #[test]
    pub fn test_jpg() -> Result<(), Box<dyn Error>> {
        do_test_success("jpg", 144, 144)
    }

    #[test]
    pub fn test_png() -> Result<(), Box<dyn Error>> {
        do_test_success("png", 144, 144)
    }

    #[test]
    pub fn test_tiff() -> Result<(), Box<dyn Error>> {
        do_test_success("tiff", 144, 144)
    }

    #[test]
    pub fn test_webp() -> Result<(), Box<dyn Error>> {
        do_test_success("webp", 144, 144)
    }
}
