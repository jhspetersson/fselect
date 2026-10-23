use chrono::{DateTime, Local};

use crate::util::variant::{Variant, VariantType};

pub fn decode_xattr(bytes: &[u8]) -> Variant {
    typed_value(bytes).unwrap_or_else(|| text(bytes))
}

fn text(bytes: &[u8]) -> Variant {
    match std::str::from_utf8(bytes) {
        Ok(value) => Variant::from_string(&value.to_string()),
        Err(_) => Variant::empty(VariantType::String),
    }
}

fn typed_value(bytes: &[u8]) -> Option<Variant> {
    let (&kind, payload) = bytes.split_first()?;
    match kind {
        0x01 => {
            let value = std::str::from_utf8(payload).ok()?;
            (!value.contains('\0')).then(|| Variant::from_string(&value.to_string()))
        }
        0x02 => Some(Variant::from_int(i64::from_le_bytes(payload.try_into().ok()?))),
        0x03 => {
            let value = f64::from_le_bytes(payload.try_into().ok()?);
            value.is_finite().then(|| Variant::from_float(value))
        }
        0x04 => match payload {
            [0] => Some(Variant::from_bool(false)),
            [1] => Some(Variant::from_bool(true)),
            _ => None,
        },
        0x05 => {
            let nanos = i64::from_le_bytes(payload.try_into().ok()?);
            let instant = DateTime::from_timestamp(nanos.div_euclid(1_000_000_000), nanos.rem_euclid(1_000_000_000) as u32)?;
            Some(Variant::from_datetime(instant.with_timezone(&Local).naive_local()))
        }
        0x06 => Some(text(payload)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(kind: u8, payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![kind];
        bytes.extend_from_slice(payload);
        bytes
    }

    #[test]
    fn plain_text_is_returned_unchanged() {
        let value = decode_xattr(b"decision");
        assert_eq!(value.get_type(), &VariantType::String);
        assert_eq!(value.to_string(), "decision");
    }

    #[test]
    fn a_string_type_byte_is_skipped() {
        let value = decode_xattr(&typed(0x01, b"decision"));
        assert_eq!(value.get_type(), &VariantType::String);
        assert_eq!(value.to_string(), "decision");
    }

    #[test]
    fn numbers_booleans_and_times_take_their_type() {
        let score = decode_xattr(&typed(0x02, &42i64.to_le_bytes()));
        assert_eq!(score.get_type(), &VariantType::Int);
        assert_eq!(score.to_int(), 42);

        let ratio = decode_xattr(&typed(0x03, &2.5f64.to_le_bytes()));
        assert_eq!(ratio.get_type(), &VariantType::Float);
        assert_eq!(ratio.to_float(), 2.5);

        assert!(decode_xattr(&typed(0x04, &[1])).to_bool());
        assert!(!decode_xattr(&typed(0x04, &[0])).to_bool());

        let nanos = 1_700_000_000_000_000_500i64;
        let stamp = decode_xattr(&typed(0x05, &nanos.to_le_bytes()));
        let expected = DateTime::from_timestamp(1_700_000_000, 500).unwrap().with_timezone(&Local).naive_local();
        assert_eq!(stamp.to_datetime().unwrap().0, expected);

        assert_eq!(decode_xattr(&typed(0x06, b"raw text")).to_string(), "raw text");
    }

    #[test]
    fn a_leading_byte_counts_as_a_type_only_when_the_payload_fits() {
        assert_eq!(decode_xattr(&[0x02, 1, 2]).to_string(), "\u{2}\u{1}\u{2}");
        assert_eq!(decode_xattr(&[0x04, 2]).to_string(), "\u{4}\u{2}");
        assert_eq!(decode_xattr(&typed(0x03, &f64::NAN.to_le_bytes())).to_string(), "");
        assert_eq!(decode_xattr(&[0x01, 0xff]).to_string(), "");
        assert_eq!(decode_xattr(&[0xff, 0xfe]).to_string(), "");
    }
}
