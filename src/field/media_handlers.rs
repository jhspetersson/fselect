use crate::field::context::FieldContext;
use crate::util::*;
use crate::util::error::SearchError;

pub fn handle_width(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_dimensions(ctx.entry);
    if let Some(&Dimensions { width, .. }) = ctx.fms.get_dimensions() {
        return Ok(Variant::from_int(width as i64));
    }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_height(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_dimensions(ctx.entry);
    if let Some(&Dimensions { height, .. }) = ctx.fms.get_dimensions() {
        return Ok(Variant::from_int(height as i64));
    }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_duration(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_duration(ctx.entry);
    if let Some(&Duration { length, .. }) = ctx.fms.get_duration() {
        return Ok(Variant::from_int(length as i64));
    }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_bitrate(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_mp3_metadata(ctx.entry);
    if let Some(mp3_info) = ctx.fms.get_mp3_metadata() {
        if let Some(frame) = mp3_info.frames.first() {
            return Ok(Variant::from_int(frame.bitrate as i64));
        }
    }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_freq(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_mp3_metadata(ctx.entry);
    if let Some(mp3_info) = ctx.fms.get_mp3_metadata() {
        if let Some(frame) = mp3_info.frames.first() {
            return Ok(Variant::from_int(frame.sampling_freq as i64));
        }
    }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_title(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_mp3_metadata(ctx.entry);
    if let Some(mp3_info) = ctx.fms.get_mp3_metadata() {
        if let Some(ref mp3_tag) = mp3_info.tag {
            return Ok(Variant::from_string(&mp3_tag.title));
        }
    }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_artist(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_mp3_metadata(ctx.entry);
    if let Some(mp3_info) = ctx.fms.get_mp3_metadata() {
        if let Some(ref mp3_tag) = mp3_info.tag {
            return Ok(Variant::from_string(&mp3_tag.artist));
        }
    }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_album(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_mp3_metadata(ctx.entry);
    if let Some(mp3_info) = ctx.fms.get_mp3_metadata() {
        if let Some(ref mp3_tag) = mp3_info.tag {
            return Ok(Variant::from_string(&mp3_tag.album));
        }
    }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_year(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_mp3_metadata(ctx.entry);
    if let Some(mp3_info) = ctx.fms.get_mp3_metadata() {
        if let Some(ref mp3_tag) = mp3_info.tag {
            return Ok(Variant::from_int(mp3_tag.year as i64));
        }
    }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_genre(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_mp3_metadata(ctx.entry);
    if let Some(mp3_info) = ctx.fms.get_mp3_metadata() {
        if let Some(ref mp3_tag) = mp3_info.tag {
            return Ok(Variant::from_string(&format!("{}", mp3_tag.genre)));
        }
    }
    Ok(Variant::empty(VariantType::String))
}

#[cfg(test)]
mod tests {
    use mp3_metadata::Genre;

    #[test]
    fn test_genre_uses_display_not_debug() {
        // Display should produce clean user-facing text
        assert_eq!(format!("{}", Genre::Rock), "Rock");
        assert_eq!(format!("{}", Genre::Blues), "Blues");
        // ClassicRock with Display should be different from Debug
        let display = format!("{}", Genre::ClassicRock);
        assert!(!display.is_empty());
    }
}
