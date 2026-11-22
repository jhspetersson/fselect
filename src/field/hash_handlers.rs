use crate::field::context::FieldContext;
use crate::util::error::SearchError;
use crate::util::{HashAlgorithm, Variant};

pub fn handle_sha1(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    let hash = ctx.fms.get_or_compute_hash(ctx.entry, HashAlgorithm::Sha1).to_string();
    Ok(Variant::from_string(&hash))
}

pub fn handle_sha256(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    let hash = ctx.fms.get_or_compute_hash(ctx.entry, HashAlgorithm::Sha256).to_string();
    Ok(Variant::from_string(&hash))
}

pub fn handle_sha512(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    let hash = ctx.fms.get_or_compute_hash(ctx.entry, HashAlgorithm::Sha512).to_string();
    Ok(Variant::from_string(&hash))
}

pub fn handle_sha3(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    let hash = ctx.fms.get_or_compute_hash(ctx.entry, HashAlgorithm::Sha3).to_string();
    Ok(Variant::from_string(&hash))
}
