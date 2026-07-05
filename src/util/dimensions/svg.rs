use std::io;
use std::path::Path;

use svg::node::element::tag::SVG;
use svg::parser::Event;

use crate::util::dimensions::DimensionsExtractor;
use crate::util::Dimensions;

pub struct SvgDimensionsExtractor;

impl SvgDimensionsExtractor {}

/// Parse an SVG length attribute: a number with an optional unit suffix
/// (`144`, `144.5`, `144px`, `10cm`). Percentages are relative and have no
/// absolute pixel value, so they yield `Ok(None)`; a value without a leading
/// number is invalid data.
fn parse_svg_length(value: &str) -> io::Result<Option<usize>> {
    let value = value.trim();
    let numeric_len = value
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(numeric_len);

    if unit.trim() == "%" {
        return Ok(None);
    }

    match number.parse::<f64>() {
        Ok(v) if v.is_finite() => Ok(Some(v.round() as usize)),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid SVG length: {}", value),
        )),
    }
}

/// Parse a `viewBox` attribute (`min-x min-y width height`, separated by
/// whitespace and/or commas) into its width/height part.
fn parse_view_box(value: &str) -> Option<Dimensions> {
    let mut parts = value
        .split(|c: char| c.is_ascii_whitespace() || c == ',')
        .filter(|p| !p.is_empty());
    let _min_x = parts.next()?;
    let _min_y = parts.next()?;
    let width = parts.next()?.parse::<f64>().ok()?;
    let height = parts.next()?.parse::<f64>().ok()?;

    if width.is_finite() && height.is_finite() && width >= 0.0 && height >= 0.0 {
        Some(Dimensions {
            width: width.round() as usize,
            height: height.round() as usize,
        })
    } else {
        None
    }
}

impl DimensionsExtractor for SvgDimensionsExtractor {
    fn supports_ext(&self, ext_lowercase: &str) -> bool {
        "svg" == ext_lowercase
    }

    fn try_read_dimensions(&self, path: &Path) -> io::Result<Option<Dimensions>> {
        let mut content = String::new();
        for event in svg::open(path, &mut content)? {
            if let Event::Tag(SVG, _, attributes) = event {
                if let (Some(width_value), Some(height_value)) =
                    (attributes.get("width"), attributes.get("height"))
                    && let (Some(width), Some(height)) =
                        (parse_svg_length(width_value)?, parse_svg_length(height_value)?)
                {
                    return Ok(Some(Dimensions { width, height }));
                }

                // No absolute width/height (the standard shape of viewBox-only
                // exports from Figma/Illustrator/icon sets, or percentage
                // sizes): fall back to the viewBox dimensions.
                return Ok(attributes.get("viewBox").and_then(|v| parse_view_box(v)));
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
    fn test_non_square() -> Result<(), Box<dyn Error>> {
        test_successful(
            SvgDimensionsExtractor,
            "image/rect.svg",
            Some(Dimensions {
                width: 200,
                height: 100,
            }),
        )
    }

    #[test]
    fn test_nonexistent_returns_error_not_panic() {
        use crate::util::dimensions::DimensionsExtractor;
        let extractor = SvgDimensionsExtractor;
        let result = extractor.try_read_dimensions(std::path::Path::new("/nonexistent/file.svg"));
        assert!(result.is_err());
    }

    #[test]
    fn test_length_units_and_floats() {
        use super::parse_svg_length;
        assert_eq!(parse_svg_length("144").unwrap(), Some(144));
        assert_eq!(parse_svg_length("144px").unwrap(), Some(144));
        assert_eq!(parse_svg_length("144.4").unwrap(), Some(144));
        assert_eq!(parse_svg_length(" 10cm ").unwrap(), Some(10));
        assert_eq!(parse_svg_length("100%").unwrap(), None);
        assert!(parse_svg_length("bar").is_err());
        assert!(parse_svg_length("").is_err());
    }

    #[test]
    fn test_view_box_parsing() {
        use super::parse_view_box;
        use crate::util::Dimensions;
        assert_eq!(
            parse_view_box("0 0 100 50"),
            Some(Dimensions { width: 100, height: 50 })
        );
        assert_eq!(
            parse_view_box("0, 0, 24, 24"),
            Some(Dimensions { width: 24, height: 24 })
        );
        assert_eq!(
            parse_view_box("-10 -10 36.5 20"),
            Some(Dimensions { width: 37, height: 20 })
        );
        assert_eq!(parse_view_box("0 0 100"), None);
        assert_eq!(parse_view_box("0 0 -1 5"), None);
    }

    #[test]
    fn test_view_box_only_svg_falls_back() -> Result<(), Box<dyn Error>> {
        use crate::util::dimensions::DimensionsExtractor;

        let tmp = std::env::temp_dir().join("fselect_test_viewbox_only.svg");
        std::fs::write(
            &tmp,
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 50"><rect/></svg>"#,
        )?;
        let result = SvgDimensionsExtractor.try_read_dimensions(&tmp);
        let _ = std::fs::remove_file(&tmp);
        assert_eq!(
            result?,
            Some(Dimensions {
                width: 100,
                height: 50
            })
        );
        Ok(())
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
