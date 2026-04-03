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
    ctx.fms.update_mime_type(ctx.entry);
    if let Some(mime) = ctx.fms.get_mime_type() {
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

    ctx.fms.update_mime_type(ctx.entry);
    if let Some(mime) = ctx.fms.get_mime_type() {
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

#[cfg(test)]
mod tests {
    use crate::config::Config;

    use super::*;

    fn is_archive(config: &Config, default_config: &Config, name: &str) -> bool {
        check_extension(name, &config.is_archive, &default_config.is_archive)
    }

    fn is_audio(config: &Config, default_config: &Config, name: &str) -> bool {
        check_extension(name, &config.is_audio, &default_config.is_audio)
    }

    fn is_book(config: &Config, default_config: &Config, name: &str) -> bool {
        check_extension(name, &config.is_book, &default_config.is_book)
    }

    fn is_doc(config: &Config, default_config: &Config, name: &str) -> bool {
        check_extension(name, &config.is_doc, &default_config.is_doc)
    }

    fn is_font(config: &Config, default_config: &Config, name: &str) -> bool {
        check_extension(name, &config.is_font, &default_config.is_font)
    }

    fn is_image(config: &Config, default_config: &Config, name: &str) -> bool {
        check_extension(name, &config.is_image, &default_config.is_image)
    }

    fn is_source(config: &Config, default_config: &Config, name: &str) -> bool {
        check_extension(name, &config.is_source, &default_config.is_source)
    }

    fn is_video(config: &Config, default_config: &Config, name: &str) -> bool {
        check_extension(name, &config.is_video, &default_config.is_video)
    }

    #[test]
    fn test_is_archive() {
        let config = Config::default();
        let default_config = Config::default();

        assert!(is_archive(&config, &default_config, "test.zip"));
        assert!(is_archive(&config, &default_config, "test.tar"));
        assert!(is_archive(&config, &default_config, "test.gz"));
        assert!(is_archive(&config, &default_config, "test.rar"));

        assert!(!is_archive(&config, &default_config, "test.txt"));
        assert!(!is_archive(&config, &default_config, "test.jpg"));
        assert!(!is_archive(&config, &default_config, "test"));
    }

    #[test]
    fn test_is_audio() {
        let config = Config::default();
        let default_config = Config::default();

        assert!(is_audio(&config, &default_config, "test.mp3"));
        assert!(is_audio(&config, &default_config, "test.wav"));
        assert!(is_audio(&config, &default_config, "test.flac"));
        assert!(is_audio(&config, &default_config, "test.ogg"));

        assert!(!is_audio(&config, &default_config, "test.txt"));
        assert!(!is_audio(&config, &default_config, "test.jpg"));
        assert!(!is_audio(&config, &default_config, "test"));
    }

    #[test]
    fn test_is_book() {
        let config = Config::default();
        let default_config = Config::default();

        assert!(is_book(&config, &default_config, "test.pdf"));
        assert!(is_book(&config, &default_config, "test.epub"));
        assert!(is_book(&config, &default_config, "test.mobi"));
        assert!(is_book(&config, &default_config, "test.djvu"));

        assert!(!is_book(&config, &default_config, "test.txt"));
        assert!(!is_book(&config, &default_config, "test.jpg"));
        assert!(!is_book(&config, &default_config, "test"));
    }

    #[test]
    fn test_is_doc() {
        let config = Config::default();
        let default_config = Config::default();

        assert!(is_doc(&config, &default_config, "test.doc"));
        assert!(is_doc(&config, &default_config, "test.docx"));
        assert!(is_doc(&config, &default_config, "test.pdf"));
        assert!(is_doc(&config, &default_config, "test.xls"));

        assert!(!is_doc(&config, &default_config, "test.txt"));
        assert!(!is_doc(&config, &default_config, "test.jpg"));
        assert!(!is_doc(&config, &default_config, "test"));
    }

    #[test]
    fn test_is_font() {
        let config = Config::default();
        let default_config = Config::default();

        assert!(is_font(&config, &default_config, "test.ttf"));
        assert!(is_font(&config, &default_config, "test.otf"));
        assert!(is_font(&config, &default_config, "test.woff"));
        assert!(is_font(&config, &default_config, "test.woff2"));

        assert!(!is_font(&config, &default_config, "test.txt"));
        assert!(!is_font(&config, &default_config, "test.jpg"));
        assert!(!is_font(&config, &default_config, "test"));
    }

    #[test]
    fn test_is_image() {
        let config = Config::default();
        let default_config = Config::default();

        assert!(is_image(&config, &default_config, "test.jpg"));
        assert!(is_image(&config, &default_config, "test.png"));
        assert!(is_image(&config, &default_config, "test.gif"));
        assert!(is_image(&config, &default_config, "test.svg"));

        assert!(!is_image(&config, &default_config, "test.txt"));
        assert!(!is_image(&config, &default_config, "test.mp3"));
        assert!(!is_image(&config, &default_config, "test"));
    }

    #[test]
    fn test_is_source() {
        let config = Config::default();
        let default_config = Config::default();

        assert!(is_source(&config, &default_config, "test.rs"));
        assert!(is_source(&config, &default_config, "test.c"));
        assert!(is_source(&config, &default_config, "test.cpp"));
        assert!(is_source(&config, &default_config, "test.java"));

        assert!(!is_source(&config, &default_config, "test.txt"));
        assert!(!is_source(&config, &default_config, "test.jpg"));
        assert!(!is_source(&config, &default_config, "test"));
    }

    #[test]
    fn test_is_video() {
        let config = Config::default();
        let default_config = Config::default();

        assert!(is_video(&config, &default_config, "test.mp4"));
        assert!(is_video(&config, &default_config, "test.avi"));
        assert!(is_video(&config, &default_config, "test.mkv"));
        assert!(is_video(&config, &default_config, "test.mov"));

        assert!(!is_video(&config, &default_config, "test.txt"));
        assert!(!is_video(&config, &default_config, "test.jpg"));
        assert!(!is_video(&config, &default_config, "test"));
    }
}
