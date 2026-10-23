use crate::field::Field;
use crate::field::context::FieldContext;
use crate::util::*;
use crate::util::datetime::parse_datetime;
use crate::util::error::SearchError;

pub fn handle_exif_datetime(ctx: &mut FieldContext, field: &Field) -> Result<Variant, SearchError> {
    ctx.fms.update_exif_metadata(ctx.entry);
    let key = match field {
        Field::ExifDateTime => "DateTime",
        Field::ExifDateTimeOriginal => "DateTimeOriginal",
        _ => return Err(SearchError::fatal(format!("Unexpected field in handle_exif_datetime: {:?}", field))),
    };

    if let Some(exif_info) = ctx.fms.get_exif_metadata()
        && let Some(exif_value) = exif_info.get(key)
            && let Ok(exif_datetime) = parse_datetime(exif_value) {
                return Ok(Variant::from_datetime(exif_datetime.0));
            }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_exif_gps_altitude(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_exif_metadata(ctx.entry);
    if let Some(exif_info) = ctx.fms.get_exif_metadata()
        && let Some(exif_value) = exif_info.get("__Alt")
            && let Ok(value) = exif_value.parse() {
                return Ok(Variant::from_float(value));
            }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_exif_gps_latitude(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_exif_metadata(ctx.entry);
    if let Some(exif_info) = ctx.fms.get_exif_metadata()
        && let Some(exif_value) = exif_info.get("__Lat")
            && let Ok(value) = exif_value.parse() {
                return Ok(Variant::from_float(value));
            }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_exif_gps_longitude(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_exif_metadata(ctx.entry);
    if let Some(exif_info) = ctx.fms.get_exif_metadata()
        && let Some(exif_value) = exif_info.get("__Lng")
            && let Ok(value) = exif_value.parse() {
                return Ok(Variant::from_float(value));
            }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_exif_string(ctx: &mut FieldContext, field: &Field) -> Result<Variant, SearchError> {
    let key = match field {
        Field::ExifMake => "Make",
        Field::ExifModel => "Model",
        Field::ExifSoftware => "Software",
        Field::ExifVersion => "ExifVersion",
        Field::ExifExposureTime => "ExposureTime",
        Field::ExifAperture => "ApertureValue",
        Field::ExifShutterSpeed => "ShutterSpeedValue",
        Field::ExifFNumber => "FNumber",
        Field::ExifIsoSpeed => "ISOSpeed",
        Field::ExifPhotographicSensitivity => "PhotographicSensitivity",
        Field::ExifFocalLength => "FocalLength",
        Field::ExifLensMake => "LensMake",
        Field::ExifLensModel => "LensModel",
        Field::ExifDescription => "ImageDescription",
        Field::ExifArtist => "Artist",
        Field::ExifCopyright => "Copyright",
        Field::ExifOrientation => "Orientation",
        Field::ExifFlash => "Flash",
        Field::ExifColorSpace => "ColorSpace",
        Field::ExifExposureProgram => "ExposureProgram",
        Field::ExifExposureBias => "ExposureBiasValue",
        Field::ExifWhiteBalance => "WhiteBalance",
        Field::ExifMeteringMode => "MeteringMode",
        Field::ExifSceneType => "SceneCaptureType",
        Field::ExifContrast => "Contrast",
        Field::ExifSaturation => "Saturation",
        Field::ExifSharpness => "Sharpness",
        Field::ExifBodySerial => "BodySerialNumber",
        Field::ExifLensSerial => "LensSerialNumber",
        Field::ExifUserComment => "UserComment",
        Field::ExifImageWidth => "PixelXDimension",
        Field::ExifImageHeight => "PixelYDimension",
        Field::ExifMaxAperture => "MaxApertureValue",
        Field::ExifDigitalZoom => "DigitalZoomRatio",
        _ => return Err(SearchError::fatal(format!("Unexpected field in handle_exif_string: {:?}", field))),
    };
    if let Some(val) = ctx.fms.get_exif_string(ctx.entry, key) {
        if field.is_numeric_field()
            && let Some(number) = Variant::from_number_text(&val.to_string())
        {
            return Ok(number);
        }
        return Ok(val);
    }
    Ok(Variant::empty(VariantType::String))
}
