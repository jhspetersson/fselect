use crate::field::context::FieldContext;
use crate::util::error::SearchError;
use crate::util::Variant;

pub fn handle_sha1(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    let hash = ctx.fms.get_or_compute_sha1(ctx.entry).to_string();
    Ok(Variant::from_string(&hash))
}

pub fn handle_sha256(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    let hash = ctx.fms.get_or_compute_sha256(ctx.entry).to_string();
    Ok(Variant::from_string(&hash))
}

pub fn handle_sha512(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    let hash = ctx.fms.get_or_compute_sha512(ctx.entry).to_string();
    Ok(Variant::from_string(&hash))
}

pub fn handle_sha3(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    let hash = ctx.fms.get_or_compute_sha3(ctx.entry).to_string();
    Ok(Variant::from_string(&hash))
}
