use crate::field::context::FieldContext;
use crate::util::*;
use crate::util::error::SearchError;

pub fn handle_sha1(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    Ok(Variant::from_string(&get_sha1_file_hash(ctx.entry)))
}

pub fn handle_sha256(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    Ok(Variant::from_string(&get_sha256_file_hash(ctx.entry)))
}

pub fn handle_sha512(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    Ok(Variant::from_string(&get_sha512_file_hash(ctx.entry)))
}

pub fn handle_sha3(ctx: &mut FieldContext) -> Result<Variant, SearchError> {
    Ok(Variant::from_string(&get_sha3_512_file_hash(ctx.entry)))
}
