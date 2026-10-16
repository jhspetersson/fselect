use std::io;
use std::path::Path;

use svg::node::element::tag::SVG;
use svg::parser::Event;

use crate::util::dimensions::DimensionsExtractor;
use crate::util::Dimensions;

pub struct SvgDimensionsExtractor;

impl SvgDimensionsExtractor {}

fn css_number_len(value: &str) -> usize {
    let bytes = value.as_bytes();
    let mut i = 0;
    if matches!(bytes.first(), Some(b'+') | Some(b'-')) {
        i += 1;
    }
    let int_start = i;
    while bytes.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
    }
    let mut digits = i - int_start;
    if bytes.get(i) == Some(&b'.') {
        let frac_start = i + 1;
        let mut j = frac_start;
        while bytes.get(j).is_some_and(u8::is_ascii_digit) {
            j += 1;
        }
        digits += j - frac_start;
        i = j;
    }
    if digits == 0 {
        return 0;
    }
    if matches!(bytes.get(i), Some(b'e') | Some(b'E')) {
        let mut j = i + 1;
        if matches!(bytes.get(j), Some(b'+') | Some(b'-')) {
            j += 1;
        }
        let exp_start = j;
        while bytes.get(j).is_some_and(u8::is_ascii_digit) {
            j += 1;
        }
        if j > exp_start {
            i = j;
        }
    }
    i
}

fn css_pixels_per_unit(unit: &str) -> Option<f64> {
    match unit.to_ascii_lowercase().as_str() {
        "" | "px" => Some(1.0),
        "in" => Some(96.0),
        "cm" => Some(96.0 / 2.54),
        "mm" => Some(96.0 / 25.4),
        "q" => Some(96.0 / 101.6),
        "pt" => Some(96.0 / 72.0),
        "pc" => Some(16.0),
        _ => None,
    }
}

/// Parse an SVG length attribute into CSS pixels: a number (`144`, `144.5`,
/// `1e2`) with an optional unit suffix (`144px`, `1in`, `10cm`). Relative
/// units (`%`, `em`, `ex`) have no absolute pixel value and yield `Ok(None)`;
/// a value without a leading number, with an unknown unit, or that is
/// negative is invalid data.
fn parse_svg_length(value: &str) -> io::Result<Option<usize>> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid SVG length: {}", value),
        )
    };
    let value = value.trim();
    let (number, unit) = value.split_at(css_number_len(value));

    if matches!(unit.to_ascii_lowercase().as_str(), "%" | "em" | "ex") {
        return Ok(None);
    }

    let number = number.parse::<f64>().map_err(|_| invalid())?;
    let scale = css_pixels_per_unit(unit).ok_or_else(invalid)?;
    let pixels = number * scale;
    if !pixels.is_finite() || pixels < 0.0 {
        return Err(invalid());
    }
    Ok(Some(pixels.round() as usize))
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
        assert_eq!(parse_svg_length("+10").unwrap(), Some(10));
        assert_eq!(parse_svg_length(".5").unwrap(), Some(1));
        assert_eq!(parse_svg_length("100%").unwrap(), None);
        assert!(parse_svg_length("bar").is_err());
        assert!(parse_svg_length("").is_err());
    }

    #[test]
    fn test_absolute_units_convert_to_css_pixels() {
        use super::parse_svg_length;
        assert_eq!(parse_svg_length("1in").unwrap(), Some(96));
        assert_eq!(parse_svg_length("2in").unwrap(), Some(192));
        assert_eq!(parse_svg_length(" 10cm ").unwrap(), Some(378));
        assert_eq!(parse_svg_length("5mm").unwrap(), Some(19));
        assert_eq!(parse_svg_length("72pt").unwrap(), Some(96));
        assert_eq!(parse_svg_length("6pc").unwrap(), Some(96));
        assert_eq!(parse_svg_length("101.6Q").unwrap(), Some(96));
        assert_eq!(parse_svg_length("1IN").unwrap(), Some(96));
    }

    #[test]
    fn test_scientific_notation() {
        use super::parse_svg_length;
        assert_eq!(parse_svg_length("1e2").unwrap(), Some(100));
        assert_eq!(parse_svg_length("2E2").unwrap(), Some(200));
        assert_eq!(parse_svg_length("1.5e1px").unwrap(), Some(15));
        assert_eq!(parse_svg_length("1e-2in").unwrap(), Some(1));
        assert!(parse_svg_length("1e").is_err());
    }

    #[test]
    fn test_relative_units_have_no_pixel_value_and_junk_is_invalid() {
        use super::parse_svg_length;
        assert_eq!(parse_svg_length("2em").unwrap(), None);
        assert_eq!(parse_svg_length("3ex").unwrap(), None);
        assert!(parse_svg_length("-5").is_err());
        assert!(parse_svg_length("10foo").is_err());
        assert!(parse_svg_length("10 px").is_err());
    }

    #[test]
    fn test_unit_and_exponent_lengths_in_file() -> Result<(), Box<dyn Error>> {
        use crate::util::dimensions::DimensionsExtractor;

        let tmp = std::env::temp_dir().join("fselect_test_svg_units.svg");
        std::fs::write(
            &tmp,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="1in" height="2e2"><rect/></svg>"#,
        )?;
        let result = SvgDimensionsExtractor.try_read_dimensions(&tmp);
        let _ = std::fs::remove_file(&tmp);
        assert_eq!(
            result?,
            Some(Dimensions {
                width: 96,
                height: 200
            })
        );
        Ok(())
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
