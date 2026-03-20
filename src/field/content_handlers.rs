use crate::field::Field;
use crate::field::context::FieldContext;
use crate::util::*;
use crate::util::error::SearchError;

pub fn handle_line_count(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_line_count(ctx.entry);
    if let Some(line_count) = ctx.fms.get_line_count() {
        return Ok(Variant::from_int(line_count as i64));
    }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_mime(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    if let Some(mime) = tree_magic_mini::from_filepath(&ctx.entry.path()) {
        return Ok(Variant::from_string(&String::from(mime)));
    }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_is_binary_or_text(ctx: &mut FieldContext, field: &Field) -> Result<Variant, SearchError> {
    ctx.fms.update_file_metadata(ctx.entry, ctx.follow_symlinks);
    if let Some(meta) = ctx.fms.get_file_metadata() {
        if meta.is_dir() {
            return Ok(Variant::from_bool(false));
        }
    }

    if let Some(mime) = tree_magic_mini::from_filepath(&ctx.entry.path()) {
        let is_text = is_text_mime(mime);
        let result = matches!(field, Field::IsText) == is_text;
        return Ok(Variant::from_bool(result));
    }

    Ok(Variant::from_bool(false))
}

pub fn handle_is_type(ctx: &mut FieldContext, field: &Field) -> Result<Variant, SearchError> {
    let os_name = ctx.entry.file_name();
    let name = match ctx.file_info {
        Some(fi) => fi.name.as_str(),
        None => &os_name.to_string_lossy(),
    };
    let result = match field {
        Field::IsArchive => check_extension(name, &ctx.config.is_archive, &ctx.default_config.is_archive),
        Field::IsAudio => check_extension(name, &ctx.config.is_audio, &ctx.default_config.is_audio),
        Field::IsBook => check_extension(name, &ctx.config.is_book, &ctx.default_config.is_book),
        Field::IsDoc => check_extension(name, &ctx.config.is_doc, &ctx.default_config.is_doc),
        Field::IsFont => check_extension(name, &ctx.config.is_font, &ctx.default_config.is_font),
        Field::IsImage => check_extension(name, &ctx.config.is_image, &ctx.default_config.is_image),
        Field::IsSource => check_extension(name, &ctx.config.is_source, &ctx.default_config.is_source),
        Field::IsVideo => check_extension(name, &ctx.config.is_video, &ctx.default_config.is_video),
        _ => unreachable!(),
    };
    Ok(Variant::from_bool(result))
}

fn check_extension(
    file_name: &str,
    config_ext: &Option<Vec<String>>,
    default_ext: &Option<Vec<String>>,
) -> bool {
    has_extension(
        file_name,
        config_ext.as_ref().unwrap_or(default_ext.as_ref().unwrap()),
    )
}
