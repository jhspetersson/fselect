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

pub fn handle_path(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    match ctx.file_info {
        Some(file_info) => {
            Ok(Variant::from_string(&file_info.name))
        }
        _ => {
            match ctx.entry.path().strip_prefix(ctx.root_path) {
                Ok(stripped_path) => {
                    Ok(Variant::from_string(&stripped_path.to_string_lossy().to_string()))
                }
                Err(_) => {
                    Ok(Variant::from_string(&ctx.entry.path().to_string_lossy().to_string()))
                }
            }
        }
    }
}

pub fn handle_abspath(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    match ctx.file_info {
        Some(file_info) => {
            Ok(Variant::from_string(&file_info.name))
        }
        _ => {
            match canonical_path(&ctx.entry.path()) {
                Ok(path) => {
                    Ok(Variant::from_string(&path))
                },
                Err(e) => {
                    Err(format!("could not get absolute path: {}", e).into())
                }
            }
        }
    }
}

pub fn handle_directory(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    let file_path = match ctx.file_info {
        Some(file_info) => file_info.name.clone(),
        _ => match ctx.entry.path().strip_prefix(ctx.root_path) {
            Ok(relative_path) => relative_path.to_string_lossy().to_string(),
            Err(_) => ctx.entry.path().to_string_lossy().to_string()
        },
    };
    let pb = PathBuf::from(file_path);
    if let Some(parent) = pb.parent() {
        return Ok(Variant::from_string(&parent.to_string_lossy().to_string()));
    }
    Ok(Variant::empty(VariantType::String))
}

pub fn handle_absdir(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    let file_path = match ctx.file_info {
        Some(file_info) => file_info.name.clone(),
        _ => ctx.entry.path().to_string_lossy().to_string(),
    };
    let pb = PathBuf::from(file_path);
    if let Some(parent) = pb.parent() {
        if ctx.file_info.is_some() {
            return Ok(Variant::from_string(&parent.to_string_lossy().to_string()));
        }

        if let Ok(path) = canonical_path(&parent.to_path_buf()) {
            return Ok(Variant::from_string(&path));
        }
    }
    Ok(Variant::empty(VariantType::String))
}
