use std::cell::RefCell;
use std::path::{Path, PathBuf};

use crate::field::context::FieldContext;
use crate::fileinfo::archived_entry_levels;
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

thread_local! {
    static CANONICAL_DIR: RefCell<Option<(PathBuf, PathBuf)>> = const { RefCell::new(None) };
    #[cfg(test)]
    static CANONICALIZE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn canonicalize_calls() -> usize {
    CANONICALIZE_CALLS.with(|calls| calls.get())
}

fn canonicalize(path: &Path) -> Result<PathBuf, String> {
    #[cfg(test)]
    CANONICALIZE_CALLS.with(|calls| calls.set(calls.get() + 1));
    canonical_path(&path.to_path_buf()).map(PathBuf::from)
}

fn canonical_dir(dir: &Path) -> Result<PathBuf, String> {
    CANONICAL_DIR.with(|cache| {
        if let Some((cached_dir, canonical)) = &*cache.borrow()
            && cached_dir == dir {
                return Ok(canonical.clone());
            }
        let canonical = canonicalize(dir)?;
        *cache.borrow_mut() = Some((dir.to_path_buf(), canonical.clone()));
        Ok(canonical)
    })
}

fn absolute_entry_path(ctx: &FieldContext) -> Result<PathBuf, SearchError> {
    let path = ctx.entry.path();
    let is_symlink = ctx.entry.file_type().is_ok_and(|file_type| file_type.is_symlink());
    let canonical = match path.parent() {
        Some(parent) if !is_symlink && !parent.as_os_str().is_empty() => {
            canonical_dir(parent).map(|dir| dir.join(ctx.entry.file_name()))
        }
        _ => canonicalize(&path),
    };
    canonical.map_err(|e| format!("could not get absolute path: {}", e).into())
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
    let mut depth = relative_entry_path(ctx).components().count() as i64;
    if let Some(file_info) = ctx.file_info {
        depth += archived_entry_levels(&file_info.name) as i64;
    }
    Ok(Variant::from_int(depth))
}

pub fn handle_absdir(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    match ctx.file_info {
        Some(file_info) => Ok(parent_string(&join_archived(absolute_entry_path(ctx)?, &file_info.name))),
        _ => match ctx.entry.path().parent().map(canonical_dir) {
            Some(Ok(path)) => Ok(path_string(&path)),
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
        field_value(entry, Some(inner), root, field)
    }

    fn field_value(entry: &fs::DirEntry, inner: Option<&str>, root: &std::path::Path, field: &Field) -> String {
        let file_info = inner.map(|inner| FileInfo { name: inner.to_string(), size: 0, mode: None, modified: None });
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
    fn abspath_canonicalizes_each_directory_once() {
        let tmp = std::env::temp_dir().join("fselect_test_abspath_cache");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("sub")).unwrap();
        for i in 0..20 {
            fs::write(tmp.join(format!("f{:02}.txt", i)), "x").unwrap();
        }
        fs::write(tmp.join("sub").join("inner.txt"), "x").unwrap();

        let before = super::canonicalize_calls();
        let mut seen = 0;
        for entry in fs::read_dir(&tmp).unwrap().flatten() {
            let expected = canonical_path(&entry.path()).unwrap();
            assert_eq!(field_value(&entry, None, &tmp, &Field::AbsPath), expected);
            assert_eq!(field_value(&entry, None, &tmp, &Field::AbsDir), canonical_path(&tmp).unwrap());
            seen += 1;
        }
        assert_eq!(seen, 21);
        assert_eq!(super::canonicalize_calls() - before, 1, "every entry of one directory must share one canonicalization");

        let inner = fs::read_dir(tmp.join("sub")).unwrap().next().unwrap().unwrap();
        assert_eq!(field_value(&inner, None, &tmp, &Field::AbsPath), canonical_path(&inner.path()).unwrap());
        assert_eq!(super::canonicalize_calls() - before, 2, "a new directory costs one more");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[cfg(unix)]
    #[test]
    fn abspath_of_a_symlink_still_resolves_to_its_target() {
        let tmp = std::env::temp_dir().join("fselect_test_abspath_symlink");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("real")).unwrap();
        fs::write(tmp.join("real").join("target.txt"), "x").unwrap();
        std::os::unix::fs::symlink(tmp.join("real").join("target.txt"), tmp.join("link.txt")).unwrap();

        let link = fs::read_dir(&tmp).unwrap().flatten().find(|e| e.file_name() == "link.txt").unwrap();
        let abspath = field_value(&link, None, &tmp, &Field::AbsPath);
        let _ = fs::remove_dir_all(&tmp);

        assert!(abspath.ends_with("/real/target.txt"), "{}", abspath);
    }

    #[test]
    fn archived_entry_depth_counts_the_archive_and_its_inner_path() {
        let tmp = std::env::temp_dir().join("fselect_test_archived_depth");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("nested")).unwrap();
        fs::write(tmp.join("nested").join("pack.zip"), "").unwrap();
        let entry = fs::read_dir(tmp.join("nested")).unwrap().next().unwrap().unwrap();

        let depth = |inner: &str| archived_field(&entry, inner, &tmp, &Field::Depth);
        let top = depth("inner.txt");
        let deep = depth("sub/deep.txt");
        let dir = depth("sub/");
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(top, "3", "nested (1) / pack.zip (2) / inner.txt (3)");
        assert_eq!(deep, "4");
        assert_eq!(dir, "3", "a directory entry's trailing slash is not a level");
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
