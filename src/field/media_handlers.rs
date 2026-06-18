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

/// Read a numeric audio property (bitrate, sample rate) into a variant.
fn audio_int(
    ctx: &mut FieldContext,
    select: impl Fn(&AudioInfo) -> Option<u32>,
) -> Result<Variant, SearchError> {
    ctx.fms.update_audio_info(ctx.entry);
    if let Some(info) = ctx.fms.get_audio_info()
        && let Some(value) = select(info) {
            return Ok(Variant::from_int(value as i64));
        }
    Ok(Variant::empty(VariantType::String))
}

/// Read a string audio tag (title, artist, ...) into a variant.
fn audio_string(
    ctx: &mut FieldContext,
    select: impl Fn(&AudioInfo) -> Option<&String>,
) -> Result<Variant, SearchError> {
    ctx.fms.update_audio_info(ctx.entry);
    if let Some(info) = ctx.fms.get_audio_info()
        && let Some(value) = select(info) {
            return Ok(Variant::from_string(value));
        }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_bitrate(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    audio_int(ctx, |info| info.bitrate)
}

pub fn handle_freq(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    audio_int(ctx, |info| info.sample_rate)
}

pub fn handle_year(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    audio_int(ctx, |info| info.year)
}

pub fn handle_title(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    audio_string(ctx, |info| info.title.as_ref())
}

pub fn handle_artist(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    audio_string(ctx, |info| info.artist.as_ref())
}

pub fn handle_album(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    audio_string(ctx, |info| info.album.as_ref())
}

pub fn handle_genre(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    audio_string(ctx, |info| info.genre.as_ref())
}

pub fn handle_comment(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    audio_string(ctx, |info| info.comment.as_ref())
}

pub fn handle_track(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    audio_string(ctx, |info| info.track.as_ref())
}

pub fn handle_disc(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    audio_string(ctx, |info| info.disc.as_ref())
}
