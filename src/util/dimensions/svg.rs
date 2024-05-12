use crate::util::dimensions::DimensionsExtractor;
use crate::util::Dimensions;
use std::io;
use std::path::Path;
use svg::node::element::tag::SVG;
use svg::parser::Event;

pub struct SvgDimensionsExtractor;

impl SvgDimensionsExtractor {}

impl DimensionsExtractor for SvgDimensionsExtractor {
    fn supports_ext(&self, ext_lowercase: &str) -> bool {
        "svg" == ext_lowercase
    }

    fn try_read_dimensions(&self, path: &Path) -> io::Result<Option<Dimensions>> {
        let mut content = String::new();
        for event in svg::open(path, &mut content).unwrap() {
            if let Event::Tag(SVG, _, attributes) = event {
                if let (Some(width_value), Some(height_value)) =
                    (attributes.get("height"), attributes.get("width"))
                {
                    let width = width_value
                        .parse::<usize>()
                        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
                    let height = height_value
                        .parse::<usize>()
                        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
                    return Ok(Some(Dimensions { width, height }));
                }
            }
        }

        Ok(None)
    }
}

#[cfg(test)]
mod test {
    use super::SvgDimensionsExtractor;
    use crate::util::dimensions::{test::test_fail, test::test_successful, Dimensions};
    use std::error::Error;
    use std::io;

    #[test]
    fn test_success() -> Result<(), Box<dyn Error>> {
        test_successful(
            SvgDimensionsExtractor,
            "image/rust-logo-blk.svg",
            Some(Dimensions {
                width: 144,
                height: 144,
            }),
        )
    }

    #[test]
    fn test_corrupted() -> Result<(), Box<dyn Error>> {
        test_fail(
            SvgDimensionsExtractor,
            "image/rust-logo-blk_corrupted.svg",
            io::ErrorKind::InvalidData,
        )
    }
}
