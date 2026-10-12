use std::path::{Path, PathBuf};

use crate::field::context::FieldContext;
use crate::util::*;
use crate::util::error::SearchError;

pub fn handle_name(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    match ctx.file_info {
        Some(file_info) => {
            let name = Path::new(&file_info.name)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| file_info.name.clone());
            Ok(Variant::from_string(&name))
        }
        _ => {
            Ok(Variant::from_string(&ctx.entry.file_name().to_string_lossy().to_string()))
        }
    }
}

pub fn handle_filename(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    match ctx.file_info {
        Some(file_info) => {
            Ok(Variant::from_string(&get_stem(&file_info.name)))
        }
        _ => {
            Ok(Variant::from_string(
                &get_stem(&ctx.entry.file_name().to_string_lossy())
                    .to_string(),
            ))
        }
    }
}

pub fn handle_extension(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    match ctx.file_info {
        Some(file_info) => {
            Ok(Variant::from_string(&get_extension(&file_info.name)))
        }
        _ => {
            Ok(Variant::from_string(
                &get_extension(&ctx.entry.file_name().to_string_lossy())
                    .to_string(),
            ))
        }
    }
}

fn relative_entry_path(ctx: &FieldContext) -> PathBuf {
    match ctx.entry.path().strip_prefix(ctx.root_path) {
        Ok(stripped_path) => stripped_path.to_path_buf(),
        Err(_) => ctx.entry.path(),
    }
}

fn absolute_entry_path(ctx: &FieldContext) -> Result<PathBuf, SearchError> {
    canonical_path(&ctx.entry.path())
        .map(PathBuf::from)
        .map_err(|e| format!("could not get absolute path: {}", e).into())
}

fn join_archived(archive: PathBuf, inner: &str) -> PathBuf {
    inner.split('/').filter(|part| !part.is_empty()).fold(archive, |path, part| path.join(part))
}

fn path_string(path: &Path) -> Variant {
    Variant::from_string(&path.to_string_lossy().to_string())
}

fn parent_string(path: &Path) -> Variant {
    match path.parent() {
        Some(parent) => path_string(parent),
        None => Variant::empty(VariantType::String),
    }
}

pub fn handle_path(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    let path = relative_entry_path(ctx);
    match ctx.file_info {
        Some(file_info) => Ok(path_string(&join_archived(path, &file_info.name))),
        _ => Ok(path_string(&path)),
    }
}

pub fn handle_abspath(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    let path = absolute_entry_path(ctx)?;
    match ctx.file_info {
        Some(file_info) => Ok(path_string(&join_archived(path, &file_info.name))),
        _ => Ok(path_string(&path)),
    }
}

pub fn handle_directory(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    let path = relative_entry_path(ctx);
    match ctx.file_info {
        Some(file_info) => Ok(parent_string(&join_archived(path, &file_info.name))),
        _ => Ok(parent_string(&path)),
    }
}

pub fn handle_depth(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    let path = ctx.entry.path();
    let relative = path.strip_prefix(ctx.root_path).unwrap_or(&path);
    Ok(Variant::from_int(relative.components().count() as i64))
}

pub fn handle_absdir(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    match ctx.file_info {
        Some(file_info) => Ok(parent_string(&join_archived(absolute_entry_path(ctx)?, &file_info.name))),
        _ => match ctx.entry.path().parent().map(|parent| canonical_path(&parent.to_path_buf())) {
            Some(Ok(path)) => Ok(Variant::from_string(&path)),
            _ => Ok(Variant::empty(VariantType::String)),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::MAIN_SEPARATOR_STR as SEP;

    use crate::config::Config;
    use crate::field::Field;
    use crate::field::context::{FieldContext, FileMetadataState};
    use crate::field::dispatch;
    use crate::fileinfo::FileInfo;
    use crate::util::canonical_path;

    fn archived_field(entry: &fs::DirEntry, inner: &str, root: &std::path::Path, field: &Field) -> String {
        let file_info = Some(FileInfo { name: inner.to_string(), size: 0, mode: None, modified: None });
        let config = Config::default();
        let default_config = Config::default();
        let mut fms = FileMetadataState::new();
        #[cfg(feature = "git")]
        let mut git_cache = crate::util::git::GitCache::new();
        #[cfg(all(unix, feature = "users"))]
        let user_cache = uzers::UsersCache::new();
        let mut ctx = FieldContext {
            entry,
            file_info: &file_info,
            root_path: root,
            fms: &mut fms,
            #[cfg(feature = "git")]
            git_cache: &mut git_cache,
            follow_symlinks: false,
            config: &config,
            default_config: &default_config,
            #[cfg(all(unix, feature = "users"))]
            user_cache: &user_cache,
        };
        dispatch::get_field_value(&mut ctx, field).unwrap().to_string()
    }

    #[test]
    fn archived_entry_path_fields_include_the_archive() {
        let tmp = std::env::temp_dir().join("fselect_test_archived_path_fields");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("nested")).unwrap();
        fs::write(tmp.join("nested").join("pack.zip"), "").unwrap();
        let entry = fs::read_dir(tmp.join("nested")).unwrap().next().unwrap().unwrap();
        let abs_zip = canonical_path(&tmp.join("nested").join("pack.zip")).unwrap();

        let deep = |field: &Field| archived_field(&entry, "sub/deep.txt", &tmp, field);
        assert_eq!(deep(&Field::Path), format!("nested{SEP}pack.zip{SEP}sub{SEP}deep.txt"));
        assert_eq!(deep(&Field::Directory), format!("nested{SEP}pack.zip{SEP}sub"));
        assert_eq!(deep(&Field::AbsPath), format!("{abs_zip}{SEP}sub{SEP}deep.txt"));
        assert_eq!(deep(&Field::AbsDir), format!("{abs_zip}{SEP}sub"));
        assert_eq!(deep(&Field::Name), "deep.txt");

        let top = |field: &Field| archived_field(&entry, "inner.txt", &tmp, field);
        assert_eq!(top(&Field::Path), format!("nested{SEP}pack.zip{SEP}inner.txt"));
        assert_eq!(top(&Field::Directory), format!("nested{SEP}pack.zip"));
        assert_eq!(top(&Field::AbsDir), abs_zip);

        let _ = fs::remove_dir_all(&tmp);
    }
}
