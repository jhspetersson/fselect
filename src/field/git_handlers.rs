//! Handlers for git-related fields.

use crate::field::context::FieldContext;
use crate::util::*;
use crate::util::error::SearchError;

#[cfg(feature = "git")]
use crate::util::git::status_to_string;

pub fn handle_is_git_repo(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    Ok(Variant::from_bool(ctx.entry.path().join(".git").exists()))
}

#[cfg(feature = "git")]
pub fn handle_is_git_tracked(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    let path = ctx.entry.path();
    Ok(Variant::from_bool(
        ctx.git_cache.is_tracked(&path).unwrap_or(false),
    ))
}

#[cfg(feature = "git")]
pub fn handle_is_gitignored(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    let path = ctx.entry.path();
    Ok(Variant::from_bool(
        ctx.git_cache.is_ignored(&path).unwrap_or(false),
    ))
}

#[cfg(feature = "git")]
pub fn handle_git_status(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    let path = ctx.entry.path();
    match ctx.git_cache.status(&path) {
        Some(status) => Ok(Variant::from_string(&status_to_string(status).to_string())),
        None => Ok(Variant::empty(VariantType::String)),
    }
}

#[cfg(feature = "git")]
pub fn handle_git_branch(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    let path = ctx.entry.path();
    match ctx.git_cache.branch(&path) {
        Some(branch) => Ok(Variant::from_string(&branch)),
        None => Ok(Variant::empty(VariantType::String)),
    }
}

#[cfg(feature = "git")]
pub fn handle_git_last_commit_hash(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    let path = ctx.entry.path();
    match ctx.git_cache.last_commit(&path) {
        Some(commit) => Ok(Variant::from_string(&commit.hash)),
        None => Ok(Variant::empty(VariantType::String)),
    }
}

#[cfg(feature = "git")]
pub fn handle_git_last_commit_date(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    let path = ctx.entry.path();
    if let Some(commit) = ctx.git_cache.last_commit(&path)
        && commit.time >= 0
        && let Some(naive) = system_time_to_naive_local(
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(commit.time as u64),
        ) {
            return Ok(Variant::from_datetime(naive));
        }
    Ok(Variant::empty(VariantType::String))
}

#[cfg(feature = "git")]
pub fn handle_git_last_commit_author(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    let path = ctx.entry.path();
    match ctx.git_cache.last_commit(&path) {
        Some(commit) => Ok(Variant::from_string(&commit.author)),
        None => Ok(Variant::empty(VariantType::String)),
    }
}
