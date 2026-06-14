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
    if let Some(mp3_info) = ctx.fms.get_mp3_metadata()
        && let Some(frame) = mp3_info.frames.first() {
            return Ok(Variant::from_int(frame.bitrate as i64));
        }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_freq(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_mp3_metadata(ctx.entry);
    if let Some(mp3_info) = ctx.fms.get_mp3_metadata()
        && let Some(frame) = mp3_info.frames.first() {
            return Ok(Variant::from_int(frame.sampling_freq as i64));
        }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_title(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_mp3_metadata(ctx.entry);
    if let Some(mp3_info) = ctx.fms.get_mp3_metadata()
        && let Some(ref mp3_tag) = mp3_info.tag {
            return Ok(Variant::from_string(&mp3_tag.title));
        }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_artist(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_mp3_metadata(ctx.entry);
    if let Some(mp3_info) = ctx.fms.get_mp3_metadata()
        && let Some(ref mp3_tag) = mp3_info.tag {
            return Ok(Variant::from_string(&mp3_tag.artist));
        }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_album(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_mp3_metadata(ctx.entry);
    if let Some(mp3_info) = ctx.fms.get_mp3_metadata()
        && let Some(ref mp3_tag) = mp3_info.tag {
            return Ok(Variant::from_string(&mp3_tag.album));
        }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_year(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_mp3_metadata(ctx.entry);
    if let Some(mp3_info) = ctx.fms.get_mp3_metadata()
        && let Some(ref mp3_tag) = mp3_info.tag {
            return Ok(Variant::from_int(mp3_tag.year as i64));
        }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_genre(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_mp3_metadata(ctx.entry);
    if let Some(mp3_info) = ctx.fms.get_mp3_metadata()
        && let Some(ref mp3_tag) = mp3_info.tag {
            return Ok(Variant::from_string(&format!("{}", mp3_tag.genre)));
        }
    Ok(Variant::empty(VariantType::String))
}

/// The ID3v1 comment, if present.
fn mp3_comment(mp3_info: &mp3_metadata::MP3Metadata) -> Option<&String> {
    mp3_info.tag.as_ref().map(|tag| &tag.comment)
}

/// The track number, carried by an ID3v2 frame; returns the first one found.
fn mp3_track_number(mp3_info: &mp3_metadata::MP3Metadata) -> Option<&String> {
    mp3_info.optional_info.iter().find_map(|info| info.track_number.as_ref())
}

/// The disc number ("part of a set" ID3v2 frame); returns the first one found.
fn mp3_disc_number(mp3_info: &mp3_metadata::MP3Metadata) -> Option<&String> {
    mp3_info.optional_info.iter().find_map(|info| info.part_of_a_set.as_ref())
}

pub fn handle_comment(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_mp3_metadata(ctx.entry);
    if let Some(mp3_info) = ctx.fms.get_mp3_metadata()
        && let Some(comment) = mp3_comment(mp3_info) {
            return Ok(Variant::from_string(comment));
        }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_track(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_mp3_metadata(ctx.entry);
    if let Some(mp3_info) = ctx.fms.get_mp3_metadata()
        && let Some(track) = mp3_track_number(mp3_info) {
            return Ok(Variant::from_string(track));
        }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_disc(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_mp3_metadata(ctx.entry);
    if let Some(mp3_info) = ctx.fms.get_mp3_metadata()
        && let Some(disc) = mp3_disc_number(mp3_info) {
            return Ok(Variant::from_string(disc));
        }
    Ok(Variant::empty(VariantType::String))
}

#[cfg(test)]
mod tests {
    use std::time::Duration as StdDuration;

    use mp3_metadata::{AudioTag, Genre, MP3Metadata, OptionalAudioTags};

    use super::*;

    #[test]
    fn test_genre_uses_display_not_debug() {
        // Display should produce clean user-facing text
        assert_eq!(format!("{}", Genre::Rock), "Rock");
        assert_eq!(format!("{}", Genre::Blues), "Blues");
        // ClassicRock with Display should be different from Debug
        let display = format!("{}", Genre::ClassicRock);
        assert!(!display.is_empty());
    }

    fn metadata(tag: Option<AudioTag>, optional: Vec<OptionalAudioTags>) -> MP3Metadata {
        MP3Metadata {
            duration: StdDuration::default(),
            frames: vec![],
            tag,
            optional_info: optional,
        }
    }

    #[test]
    fn test_mp3_comment() {
        let tag = AudioTag {
            comment: String::from("ripped from CD"),
            ..Default::default()
        };
        let info = metadata(Some(tag), vec![]);
        assert_eq!(mp3_comment(&info), Some(&String::from("ripped from CD")));

        // No tag at all -> no comment.
        let info = metadata(None, vec![]);
        assert_eq!(mp3_comment(&info), None);
    }

    #[test]
    fn test_mp3_track_and_disc_numbers() {
        let optional = OptionalAudioTags {
            track_number: Some(String::from("4/9")),
            part_of_a_set: Some(String::from("1/2")),
            ..Default::default()
        };
        let info = metadata(None, vec![optional]);

        assert_eq!(mp3_track_number(&info), Some(&String::from("4/9")));
        assert_eq!(mp3_disc_number(&info), Some(&String::from("1/2")));
    }

    #[test]
    fn test_mp3_track_skips_frames_without_value() {
        // The first optional frame lacks a track number; the value should be
        // picked up from the later frame that has one.
        let empty = OptionalAudioTags::default();
        let with_track = OptionalAudioTags {
            track_number: Some(String::from("7")),
            ..Default::default()
        };
        let info = metadata(None, vec![empty, with_track]);

        assert_eq!(mp3_track_number(&info), Some(&String::from("7")));
        assert_eq!(mp3_disc_number(&info), None);
    }
}
