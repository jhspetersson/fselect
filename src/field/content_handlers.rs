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

pub fn handle_word_count(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_content_stats(ctx.entry);
    if let Some(stats) = ctx.fms.get_content_stats()
        && stats.is_text {
            return Ok(Variant::from_int(stats.word_count as i64));
        }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_char_count(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_content_stats(ctx.entry);
    if let Some(stats) = ctx.fms.get_content_stats()
        && stats.is_text {
            return Ok(Variant::from_int(stats.char_count as i64));
        }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_encoding(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_content_stats(ctx.entry);
    if let Some(stats) = ctx.fms.get_content_stats()
        && stats.is_text {
            return Ok(Variant::from_string(&stats.encoding));
        }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_has_bom(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    if let Some(stats) = ctx.fms.get_content_stats() {
        return Ok(Variant::from_bool(stats.has_bom));
    }
    Ok(Variant::from_bool(has_bom(ctx.entry)))
}

pub fn handle_line_ending(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_content_stats(ctx.entry);
    if let Some(stats) = ctx.fms.get_content_stats()
        && stats.is_text {
            return Ok(Variant::from_string(&stats.line_ending));
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
    if let Some(meta) = ctx.fms.get_file_metadata()
        && meta.is_dir() {
            return Ok(Variant::from_bool(false));
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
        _ => return Err(SearchError::fatal(format!("Unexpected field in handle_is_type: {:?}", field))),
    };
    Ok(Variant::from_bool(result))
}

fn check_extension(
    file_name: &str,
    config_ext: &Option<Vec<String>>,
    default_ext: &Option<Vec<String>>,
) -> bool {
    match config_ext.as_ref().or(default_ext.as_ref()) {
        Some(extensions) => has_extension(file_name, extensions),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use crate::config::Config;

    use super::*;

    fn test_field(entry: &fs::DirEntry, root_path: &Path, field: &Field) -> Variant {
        let config = Config::default();
        let default_config = Config::default();
        let mut fms = crate::field::context::FileMetadataState::new();
        #[cfg(feature = "git")]
        let mut git_cache = crate::util::git::GitCache::new();
        #[cfg(all(unix, feature = "users"))]
        let user_cache = uzers::UsersCache::new();
        let none_file_info = None;
        let mut ctx = FieldContext {
            entry,
            file_info: &none_file_info,
            root_path,
            fms: &mut fms,
            #[cfg(feature = "git")]
            git_cache: &mut git_cache,
            follow_symlinks: true,
            config: &config,
            default_config: &default_config,
            #[cfg(all(unix, feature = "users"))]
            user_cache: &user_cache,
        };
        crate::field::dispatch::get_field_value(&mut ctx, field).unwrap()
    }

    fn entry_for(dir: &Path, name: &str) -> fs::DirEntry {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_name() == name)
            .unwrap()
    }

    #[test]
    fn test_content_fields_on_text_file() {
        let tmp = std::env::temp_dir().join("fselect_test_content_fields_text_h");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.txt"), "hello world\nsecond line\n").unwrap();

        let entry = entry_for(&tmp, "a.txt");

        assert_eq!(test_field(&entry, &tmp, &Field::WordCount).to_string(), "4");
        assert_eq!(test_field(&entry, &tmp, &Field::CharCount).to_string(), "24");
        assert_eq!(test_field(&entry, &tmp, &Field::Encoding).to_string(), "ASCII");
        assert_eq!(test_field(&entry, &tmp, &Field::HasBom).to_string(), "false");
        assert_eq!(test_field(&entry, &tmp, &Field::LineEnding).to_string(), "LF");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_content_fields_on_binary_file() {
        let tmp = std::env::temp_dir().join("fselect_test_content_fields_binary_h");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.bin"), [0x00u8, 0x01, 0x02, b'x']).unwrap();

        let entry = entry_for(&tmp, "a.bin");

        // Text-only fields surface an empty value for binary content.
        assert_eq!(test_field(&entry, &tmp, &Field::WordCount).to_string(), "");
        assert_eq!(test_field(&entry, &tmp, &Field::CharCount).to_string(), "");
        assert_eq!(test_field(&entry, &tmp, &Field::Encoding).to_string(), "");
        assert_eq!(test_field(&entry, &tmp, &Field::LineEnding).to_string(), "");
        // has_bom is still a definite boolean.
        assert_eq!(test_field(&entry, &tmp, &Field::HasBom).to_string(), "false");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_has_bom_true_for_utf8_bom_file() {
        let tmp = std::env::temp_dir().join("fselect_test_content_fields_bom_h");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let mut content = vec![0xEFu8, 0xBB, 0xBF];
        content.extend_from_slice(b"data");
        fs::write(tmp.join("bom.txt"), content).unwrap();

        let entry = entry_for(&tmp, "bom.txt");

        assert_eq!(test_field(&entry, &tmp, &Field::HasBom).to_string(), "true");
        assert_eq!(test_field(&entry, &tmp, &Field::Encoding).to_string(), "UTF-8");

        let _ = fs::remove_dir_all(&tmp);
    }

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
    fn test_check_extension_both_none() {
        assert!(!check_extension("test.zip", &None, &None));
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
