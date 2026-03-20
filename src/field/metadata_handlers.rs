#![allow(unused_imports, unused_variables)]

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use chrono::{DateTime, Local};
#[cfg(unix)]
use xattr::FileExt;

use crate::field::context::FieldContext;
use crate::mode;
use crate::util::*;
use crate::util::error::SearchError;

pub fn handle_size(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    match ctx.file_info {
        Some(file_info) => {
            Ok(Variant::from_int(file_info.size as i64))
        }
        _ => {
            ctx.fms.update_file_metadata(ctx.entry, ctx.follow_symlinks);
            if let Some(attrs) = ctx.fms.get_file_metadata() {
                return Ok(Variant::from_int(attrs.len() as i64));
            }
            Ok(Variant::empty(VariantType::String))
        }
    }
}

pub fn handle_formatted_size(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    match ctx.file_info {
        Some(file_info) => {
            Ok(Variant::from_string(&format_filesize(
                file_info.size,
                ctx.config
                    .default_file_size_format
                    .as_ref()
                    .unwrap_or(&String::new()),
            )?))
        }
        _ => {
            ctx.fms.update_file_metadata(ctx.entry, ctx.follow_symlinks);
            if let Some(attrs) = ctx.fms.get_file_metadata() {
                return Ok(Variant::from_string(&format_filesize(
                    attrs.len(),
                    ctx.config
                        .default_file_size_format
                        .as_ref()
                        .unwrap_or(&String::new()),
                )?));
            }
            Ok(Variant::empty(VariantType::String))
        }
    }
}

pub fn handle_is_dir(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    match ctx.file_info {
        Some(file_info) => {
            Ok(Variant::from_bool(
                file_info.name.ends_with('/') || file_info.name.ends_with('\\'),
            ))
        }
        _ => {
            ctx.fms.update_file_metadata(ctx.entry, ctx.follow_symlinks);
            if let Some(attrs) = ctx.fms.get_file_metadata() {
                return Ok(Variant::from_bool(attrs.is_dir()));
            }
            Ok(Variant::empty(VariantType::String))
        }
    }
}

pub fn handle_is_file(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    match ctx.file_info {
        Some(file_info) => {
            Ok(Variant::from_bool(!file_info.name.ends_with('/') && !file_info.name.ends_with('\\')))
        }
        _ => {
            ctx.fms.update_file_metadata(ctx.entry, ctx.follow_symlinks);
            if let Some(attrs) = ctx.fms.get_file_metadata() {
                return Ok(Variant::from_bool(attrs.is_file()));
            }
            Ok(Variant::empty(VariantType::String))
        }
    }
}

pub fn handle_is_symlink(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    match ctx.file_info {
        Some(_) => {
            Ok(Variant::from_bool(false))
        }
        _ => {
            if let Some(meta) = get_metadata(ctx.entry, false) {
                return Ok(Variant::from_bool(meta.file_type().is_symlink()));
            }
            Ok(Variant::empty(VariantType::String))
        }
    }
}

pub fn handle_is_pipe(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    Ok(check_file_mode(ctx, &mode::is_pipe, &mode::mode_is_pipe))
}

pub fn handle_is_char_device(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    Ok(check_file_mode(ctx, &mode::is_char_device, &mode::mode_is_char_device))
}

pub fn handle_is_block_device(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    Ok(check_file_mode(ctx, &mode::is_block_device, &mode::mode_is_block_device))
}

pub fn handle_is_socket(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    Ok(check_file_mode(ctx, &mode::is_socket, &mode::mode_is_socket))
}

pub fn handle_device(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    #[cfg(unix)]
    {
        ctx.fms.update_file_metadata(ctx.entry, ctx.follow_symlinks);
        if let Some(attrs) = ctx.fms.get_file_metadata() {
            return Ok(Variant::from_int(attrs.dev() as i64));
        }
    }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_rdev(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    #[cfg(unix)]
    {
        ctx.fms.update_file_metadata(ctx.entry, ctx.follow_symlinks);
        if let Some(attrs) = ctx.fms.get_file_metadata() {
            return Ok(Variant::from_int(attrs.rdev() as i64));
        }
    }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_inode(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    #[cfg(unix)]
    {
        ctx.fms.update_file_metadata(ctx.entry, ctx.follow_symlinks);
        if let Some(attrs) = ctx.fms.get_file_metadata() {
            return Ok(Variant::from_int(attrs.ino() as i64));
        }
    }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_blocks(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    #[cfg(unix)]
    {
        ctx.fms.update_file_metadata(ctx.entry, ctx.follow_symlinks);
        if let Some(attrs) = ctx.fms.get_file_metadata() {
            return Ok(Variant::from_int(attrs.blocks() as i64));
        }
    }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_hardlinks(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    #[cfg(unix)]
    {
        ctx.fms.update_file_metadata(ctx.entry, ctx.follow_symlinks);
        if let Some(attrs) = ctx.fms.get_file_metadata() {
            return Ok(Variant::from_int(attrs.nlink() as i64));
        }
    }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_atime(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    #[cfg(unix)]
    {
        ctx.fms.update_file_metadata(ctx.entry, ctx.follow_symlinks);
        if let Some(attrs) = ctx.fms.get_file_metadata() {
            return Ok(Variant::from_int(attrs.atime()));
        }
    }
    Ok(Variant::empty(VariantType::Int))
}

pub fn handle_mtime(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    #[cfg(unix)]
    {
        ctx.fms.update_file_metadata(ctx.entry, ctx.follow_symlinks);
        if let Some(attrs) = ctx.fms.get_file_metadata() {
            return Ok(Variant::from_int(attrs.mtime()));
        }
    }
    Ok(Variant::empty(VariantType::Int))
}

pub fn handle_ctime(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    #[cfg(unix)]
    {
        ctx.fms.update_file_metadata(ctx.entry, ctx.follow_symlinks);
        if let Some(attrs) = ctx.fms.get_file_metadata() {
            return Ok(Variant::from_int(attrs.ctime()));
        }
    }
    Ok(Variant::empty(VariantType::Int))
}

pub fn handle_created(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_file_metadata(ctx.entry, ctx.follow_symlinks);
    if let Some(attrs) = ctx.fms.get_file_metadata() {
        if let Ok(sdt) = attrs.created() {
            let dt: DateTime<Local> = DateTime::from(sdt);
            return Ok(Variant::from_datetime(dt.naive_local()));
        }
    }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_accessed(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_file_metadata(ctx.entry, ctx.follow_symlinks);
    if let Some(attrs) = ctx.fms.get_file_metadata() {
        if let Ok(sdt) = attrs.accessed() {
            let dt: DateTime<Local> = DateTime::from(sdt);
            return Ok(Variant::from_datetime(dt.naive_local()));
        }
    }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_modified(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    match ctx.file_info {
        Some(file_info) => {
            if let Some(file_info_modified) = &file_info.modified {
                let dt = to_local_datetime(file_info_modified);
                return Ok(Variant::from_datetime(dt));
            }
            Ok(Variant::empty(VariantType::String))
        }
        _ => {
            ctx.fms.update_file_metadata(ctx.entry, ctx.follow_symlinks);
            if let Some(attrs) = ctx.fms.get_file_metadata() {
                if let Ok(sdt) = attrs.modified() {
                    let dt: DateTime<Local> = DateTime::from(sdt);
                    return Ok(Variant::from_datetime(dt.naive_local()));
                }
            }
            Ok(Variant::empty(VariantType::String))
        }
    }
}

pub fn handle_is_hidden(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    match ctx.file_info {
        Some(file_info) => {
            Ok(Variant::from_bool(is_hidden(&file_info.name, &None, true)))
        }
        _ => {
            ctx.fms.update_file_metadata(ctx.entry, ctx.follow_symlinks);
            Ok(Variant::from_bool(is_hidden(
                &ctx.entry.file_name().to_string_lossy(),
                ctx.fms.get_file_metadata_as_option(),
                false,
            )))
        }
    }
}

pub fn handle_is_empty(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    match ctx.file_info {
        Some(file_info) => {
            Ok(Variant::from_bool(file_info.size == 0))
        }
        _ => {
            ctx.fms.update_file_metadata(ctx.entry, ctx.follow_symlinks);
            if let Some(attrs) = ctx.fms.get_file_metadata() {
                return match attrs.is_dir() {
                    true => match is_dir_empty(ctx.entry) {
                        Some(result) => Ok(Variant::from_bool(result)),
                        None => Ok(Variant::empty(VariantType::Bool)),
                    },
                    false => Ok(Variant::from_bool(attrs.len() == 0)),
                };
            }
            Ok(Variant::empty(VariantType::String))
        }
    }
}

pub fn handle_has_xattrs(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    #[cfg(unix)]
    {
        if let Ok(file) = fs::File::open(ctx.entry.path()) {
            if let Ok(xattrs) = file.list_xattr() {
                let has_xattrs = xattrs.count() > 0;
                return Ok(Variant::from_bool(has_xattrs));
            }
        }
    }

    #[cfg(windows)]
    {
        return Ok(Variant::from_bool(
            crate::util::win_xattr::has_any_ads(&ctx.entry.path()),
        ));
    }

    Ok(Variant::empty(VariantType::Bool))
}

pub fn handle_xattr_count(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    #[cfg(unix)]
    {
        if let Ok(file) = fs::File::open(ctx.entry.path()) {
            if let Ok(xattrs) = file.list_xattr() {
                return Ok(Variant::from_int(xattrs.count() as i64));
            }
        }
    }

    #[cfg(windows)]
    {
        return Ok(Variant::from_int(
            crate::util::win_xattr::count_ads(&ctx.entry.path()) as i64,
        ));
    }

    Ok(Variant::empty(VariantType::Int))
}

pub fn handle_extattrs(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(file) = fs::File::open(ctx.entry.path()) {
            if let Some(flags) = crate::util::extattrs::get_ext_attrs(&file) {
                return Ok(Variant::from_string(
                    &crate::util::extattrs::format_ext_attrs(flags),
                ));
            }
        }
    }

    Ok(Variant::empty(VariantType::String))
}

pub fn handle_has_extattrs(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(file) = fs::File::open(ctx.entry.path()) {
            if let Some(flags) = crate::util::extattrs::get_ext_attrs(&file) {
                return Ok(Variant::from_bool(flags != 0));
            }
        }
    }

    Ok(Variant::empty(VariantType::Bool))
}

pub fn handle_acl(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(file) = fs::File::open(ctx.entry.path()) {
            if let Ok(Some(acl_data)) = file.get_xattr("system.posix_acl_access") {
                if let Some(entries) = crate::util::acl::parse_acl(&acl_data) {
                    return Ok(Variant::from_string(&crate::util::acl::format_acl(&entries)));
                }
            }
        }
    }

    #[cfg(windows)]
    {
        if let Some(acl_str) = crate::util::win_acl::format_acl(&ctx.entry.path()) {
            return Ok(Variant::from_string(&acl_str));
        }
    }

    Ok(Variant::empty(VariantType::String))
}

pub fn handle_has_acl(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(file) = fs::File::open(ctx.entry.path()) {
            if let Ok(Some(acl_data)) = file.get_xattr("system.posix_acl_access") {
                if let Some(entries) = crate::util::acl::parse_acl(&acl_data) {
                    return Ok(Variant::from_bool(!entries.is_empty()));
                }
            }
        }
    }

    #[cfg(windows)]
    {
        return Ok(Variant::from_bool(
            crate::util::win_acl::has_explicit_acl(&ctx.entry.path()),
        ));
    }

    Ok(Variant::empty(VariantType::Bool))
}

pub fn handle_default_acl(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    #[cfg(target_os = "linux")]
    {
        if ctx.entry.path().is_dir() {
            if let Ok(file) = fs::File::open(ctx.entry.path()) {
                if let Ok(Some(acl_data)) = file.get_xattr("system.posix_acl_default") {
                    if let Some(entries) = crate::util::acl::parse_acl(&acl_data) {
                        return Ok(Variant::from_string(&crate::util::acl::format_acl(&entries)));
                    }
                }
            }
        }
    }

    Ok(Variant::empty(VariantType::String))
}

pub fn handle_has_default_acl(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    #[cfg(target_os = "linux")]
    {
        if ctx.entry.path().is_dir() {
            if let Ok(file) = fs::File::open(ctx.entry.path()) {
                if let Ok(Some(acl_data)) = file.get_xattr("system.posix_acl_default") {
                    if let Some(entries) = crate::util::acl::parse_acl(&acl_data) {
                        return Ok(Variant::from_bool(!entries.is_empty()));
                    }
                }
            }
        }
    }

    Ok(Variant::empty(VariantType::Bool))
}

pub fn handle_has_capabilities(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(file) = fs::File::open(ctx.entry.path()) {
            if let Ok(caps_xattr) = file.get_xattr("security.capability") {
                return Ok(Variant::from_bool(caps_xattr.is_some()));
            }
        }
    }

    Ok(Variant::empty(VariantType::Bool))
}

pub fn handle_capabilities(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(file) = fs::File::open(ctx.entry.path()) {
            if let Ok(Some(caps_xattr)) = file.get_xattr("security.capability") {
                let caps_string =
                    crate::util::capabilities::parse_capabilities(caps_xattr);
                return Ok(Variant::from_string(&caps_string));
            }
        }
    }

    Ok(Variant::empty(VariantType::String))
}

pub fn handle_is_shebang(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    Ok(Variant::from_bool(is_shebang(&ctx.entry.path())))
}

pub fn check_file_mode(
    ctx: &mut FieldContext,
    mode_func_boxed: &dyn Fn(&std::fs::Metadata) -> bool,
    mode_func_i32: &dyn Fn(u32) -> bool,
) -> Variant {
    match ctx.file_info {
        Some(file_info) => {
            if let Some(mode) = file_info.mode {
                return Variant::from_bool(mode_func_i32(mode));
            }
        }
        _ => {
            ctx.fms.update_file_metadata(ctx.entry, ctx.follow_symlinks);
            if let Some(attrs) = ctx.fms.get_file_metadata() {
                return Variant::from_bool(mode_func_boxed(attrs));
            }
        }
    }
    Variant::from_bool(false)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use crate::config::Config;
    use crate::field::Field;
    use crate::field::context::{FieldContext, FileMetadataState};
    use crate::field::dispatch;
    use crate::fileinfo::FileInfo;

    fn test_field(
        entry: &fs::DirEntry,
        file_info: &Option<FileInfo>,
        root_path: &Path,
        field: &Field,
    ) -> crate::util::Variant {
        let config = Config::default();
        let default_config = Config::default();
        let mut fms = FileMetadataState::new();
        #[cfg(all(unix, feature = "users"))]
        let user_cache = uzers::UsersCache::new();
        let mut ctx = FieldContext {
            entry,
            file_info,
            root_path,
            fms: &mut fms,
            follow_symlinks: true,
            config: &config,
            default_config: &default_config,
            #[cfg(all(unix, feature = "users"))]
            user_cache: &user_cache,
        };
        dispatch::get_field_value(&mut ctx, field).unwrap()
    }

    #[test]
    fn test_is_file_false_for_backslash_terminated_archive_entry() {
        let tmp = std::env::temp_dir().join("fselect_test_isfile_backslash_h");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("dummy.txt"), "").unwrap();

        let entry = fs::read_dir(&tmp).unwrap().next().unwrap().unwrap();
        let file_info = Some(FileInfo {
            name: String::from("somedir\\"),
            size: 0,
            mode: None,
            modified: None,
        });

        let result = test_field(&entry, &file_info, &tmp, &Field::IsFile);
        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(result.to_string(), "false");
    }

    #[test]
    fn test_is_dir_and_is_file_consistent_for_backslash_archive_entry() {
        let tmp = std::env::temp_dir().join("fselect_test_consistency_backslash_h");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("dummy.txt"), "").unwrap();

        let entry = fs::read_dir(&tmp).unwrap().next().unwrap().unwrap();
        let file_info = Some(FileInfo {
            name: String::from("somedir\\"),
            size: 0,
            mode: None,
            modified: None,
        });

        let is_dir = test_field(&entry, &file_info, &tmp, &Field::IsDir);
        let is_file = test_field(&entry, &file_info, &tmp, &Field::IsFile);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(is_dir.to_string(), "true");
        assert_eq!(is_file.to_string(), "false");
    }

    #[test]
    fn test_is_symlink_true_when_following_symlinks() {
        let tmp = std::env::temp_dir().join("fselect_test_symlink_follow_islink_h");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("real_file.txt"), "hello world").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink("real_file.txt", tmp.join("link")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(tmp.join("real_file.txt"), tmp.join("link")).unwrap();

        let entry = fs::read_dir(&tmp)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_name() == "link")
            .unwrap();

        let result = test_field(&entry, &None, &tmp, &Field::IsSymlink);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(
            result.to_string(),
            "true",
            "is_symlink should be true for a symlink even when follow_symlinks is on"
        );
    }

    #[test]
    fn test_size_follows_symlink_when_requested() {
        let tmp = std::env::temp_dir().join("fselect_test_symlink_follow_size_h");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let content = "hello world, this is a reasonably long test string for size comparison";
        fs::write(tmp.join("real_file.txt"), content).unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink("real_file.txt", tmp.join("link")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(tmp.join("real_file.txt"), tmp.join("link")).unwrap();

        let entry = fs::read_dir(&tmp)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_name() == "link")
            .unwrap();

        let result = test_field(&entry, &None, &tmp, &Field::Size);
        let _ = fs::remove_dir_all(&tmp);

        let size = result.to_int();
        let expected_size = content.len() as i64;

        assert_eq!(
            size, expected_size,
            "size should be target file's size ({}) when following symlinks, got {}",
            expected_size, size
        );
    }
}
