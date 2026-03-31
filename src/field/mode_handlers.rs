use crate::field::context::FieldContext;
use crate::field::metadata_handlers::check_file_mode;
use crate::mode;
use crate::util::*;
use crate::util::error::SearchError;

pub fn handle_mode(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    match ctx.file_info {
        Some(file_info) => {
            if let Some(mode) = file_info.mode {
                return Ok(Variant::from_string(&mode::format_unix_mode(mode)));
            }
            Ok(Variant::empty(VariantType::String))
        }
        _ => {
            ctx.fms.update_file_metadata(ctx.entry, ctx.follow_symlinks);
            if let Some(attrs) = ctx.fms.get_file_metadata() {
                return Ok(Variant::from_string(&mode::get_mode(attrs)));
            }
            Ok(Variant::empty(VariantType::String))
        }
    }
}

pub fn handle_user_read(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    Ok(check_file_mode(ctx, &mode::user_read, &mode::mode_user_read))
}

pub fn handle_user_write(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    Ok(check_file_mode(ctx, &mode::user_write, &mode::mode_user_write))
}

pub fn handle_user_exec(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    Ok(check_file_mode(ctx, &mode::user_exec, &mode::mode_user_exec))
}

pub fn handle_user_all(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    Ok(check_file_mode(ctx, &mode::user_all, &mode::mode_user_all))
}

pub fn handle_group_read(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    Ok(check_file_mode(ctx, &mode::group_read, &mode::mode_group_read))
}

pub fn handle_group_write(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    Ok(check_file_mode(ctx, &mode::group_write, &mode::mode_group_write))
}

pub fn handle_group_exec(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    Ok(check_file_mode(ctx, &mode::group_exec, &mode::mode_group_exec))
}

pub fn handle_group_all(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    Ok(check_file_mode(ctx, &mode::group_all, &mode::mode_group_all))
}

pub fn handle_other_read(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    Ok(check_file_mode(ctx, &mode::other_read, &mode::mode_other_read))
}

pub fn handle_other_write(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    Ok(check_file_mode(ctx, &mode::other_write, &mode::mode_other_write))
}

pub fn handle_other_exec(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    Ok(check_file_mode(ctx, &mode::other_exec, &mode::mode_other_exec))
}

pub fn handle_other_all(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    Ok(check_file_mode(ctx, &mode::other_all, &mode::mode_other_all))
}

pub fn handle_suid(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    Ok(check_file_mode(ctx, &mode::suid_bit_set, &mode::mode_suid))
}

pub fn handle_sgid(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    Ok(check_file_mode(ctx, &mode::sgid_bit_set, &mode::mode_sgid))
}

pub fn handle_is_sticky(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    Ok(check_file_mode(ctx, &mode::sticky_bit_set, &mode::mode_sticky))
}

pub fn handle_uid(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_file_metadata(ctx.entry, ctx.follow_symlinks);
    if let Some(attrs) = ctx.fms.get_file_metadata() {
        if let Some(uid) = mode::get_uid(attrs) {
            return Ok(Variant::from_int(uid as i64));
        }
    }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_gid(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    ctx.fms.update_file_metadata(ctx.entry, ctx.follow_symlinks);
    if let Some(attrs) = ctx.fms.get_file_metadata() {
        if let Some(gid) = mode::get_gid(attrs) {
            return Ok(Variant::from_int(gid as i64));
        }
    }
    Ok(Variant::empty(VariantType::String))
}

#[cfg(all(unix, feature = "users"))]
pub fn handle_user(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    use uzers::Users;

    ctx.fms.update_file_metadata(ctx.entry, ctx.follow_symlinks);
    if let Some(attrs) = ctx.fms.get_file_metadata() {
        if let Some(uid) = mode::get_uid(attrs) {
            if let Some(user) = ctx.user_cache.get_user_by_uid(uid) {
                return Ok(Variant::from_string(
                    &user.name().to_string_lossy().to_string(),
                ));
            }
        }
    }
    Ok(Variant::empty(VariantType::String))
}

#[cfg(all(unix, feature = "users"))]
pub fn handle_group(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    use uzers::Groups;

    ctx.fms.update_file_metadata(ctx.entry, ctx.follow_symlinks);
    if let Some(attrs) = ctx.fms.get_file_metadata() {
        if let Some(gid) = mode::get_gid(attrs) {
            if let Some(group) = ctx.user_cache.get_group_by_gid(gid) {
                return Ok(Variant::from_string(
                    &group.name().to_string_lossy().to_string(),
                ));
            }
        }
    }
    Ok(Variant::empty(VariantType::String))
}
