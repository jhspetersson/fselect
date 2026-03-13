use std::fmt::{Display, Error, Formatter};

use chrono::NaiveDateTime;

use crate::util::{format_datetime, parse_datetime, parse_filesize, str_to_bool};

#[derive(Clone, Debug)]
pub enum VariantType {
    String,
    Int,
    Float,
    Bool,
    DateTime,
}

#[derive(Clone, Debug)]
pub struct Variant {
    value_type: VariantType,
    string_value: String,
    int_value: Option<i64>,
    float_value: Option<f64>,
    bool_value: Option<bool>,
    dt_from: Option<NaiveDateTime>,
    dt_to: Option<NaiveDateTime>,
}

impl Variant {
    pub fn empty(value_type: VariantType) -> Variant {
        Variant {
            value_type,
            string_value: String::new(),
            int_value: None,
            float_value: None,
            bool_value: None,
            dt_from: None,
            dt_to: None,
        }
    }

    pub fn get_type(&self) -> &VariantType {
        &self.value_type
    }

    pub fn from_int(value: i64) -> Variant {
        Variant {
            value_type: VariantType::Int,
            string_value: format!("{}", value),
            int_value: Some(value),
            float_value: Some(value as f64),
            bool_value: None,
            dt_from: None,
            dt_to: None,
        }
    }

    pub fn from_float(value: f64) -> Variant {
        if value.is_nan() || value.is_infinite() {
            return Variant::empty(VariantType::Float);
        }
        let value = value + 0.0;
        Variant {
            value_type: VariantType::Float,
            string_value: format!("{}", value),
            int_value: Some(value as i64),
            float_value: Some(value),
            bool_value: None,
            dt_from: None,
            dt_to: None,
        }
    }

    pub fn from_string(value: &String) -> Variant {
        Variant {
            value_type: VariantType::String,
            string_value: value.to_owned(),
            int_value: None,
            float_value: None,
            bool_value: None,
            dt_from: None,
            dt_to: None,
        }
    }

    pub fn from_signed_string(value: &String, minus: bool) -> Variant {
        let string_value = match minus {
            true => {
                let mut result = String::from("-");
                result += &value.to_owned();

                result
            }
            false => value.to_owned(),
        };

        Variant {
            value_type: VariantType::String,
            string_value,
            int_value: None,
            float_value: None,
            bool_value: None,
            dt_from: None,
            dt_to: None,
        }
    }

    pub fn from_bool(value: bool) -> Variant {
        Variant {
            value_type: VariantType::Bool,
            string_value: match value {
                true => String::from("true"),
                _ => String::from("false"),
            },
            int_value: match value {
                true => Some(1),
                _ => Some(0),
            },
            float_value: None,
            bool_value: Some(value),
            dt_from: None,
            dt_to: None,
        }
    }

    pub fn from_datetime(value: NaiveDateTime) -> Variant {
        Variant {
            value_type: VariantType::DateTime,
            string_value: format_datetime(&value),
            int_value: Some(0),
            float_value: None,
            bool_value: None,
            dt_from: Some(value),
            dt_to: Some(value),
        }
    }

    pub fn to_string(&self) -> String {
        self.string_value.to_owned()
    }

    pub fn to_int(&self) -> i64 {
        match self.int_value {
            Some(i) => i,
            None => {
                if self.float_value.is_some() {
                    return self.float_value.unwrap() as i64;
                }

                let int_value = self.string_value.parse::<usize>();
                match int_value {
                    Ok(i) => i as i64,
                    _ => match parse_filesize(&self.string_value) {
                        Some(size) => size as i64,
                        _ => 0,
                    },
                }
            }
        }
    }

    pub fn to_float(&self) -> f64 {
        if self.float_value.is_some() {
            return self.float_value.unwrap();
        }

        match self.int_value {
            Some(i) => i as f64,
            None => {
                let float_value = self.string_value.parse::<f64>();
                match float_value {
                    Ok(f) => f,
                    _ => match parse_filesize(&self.string_value) {
                        Some(size) => size as f64,
                        _ => 0.0,
                    },
                }
            }
        }
    }

    pub fn to_bool(&self) -> bool {
        if let Some(value) = self.bool_value {
            value
        } else if !self.string_value.is_empty() {
            str_to_bool(&self.string_value).unwrap_or(false)
        } else if let Some(int_value) = self.int_value {
            int_value == 1
        } else if let Some(float_value) = self.float_value {
            float_value == 1.0
        } else {
            false
        }
    }

    pub fn to_datetime(&self) -> Result<(NaiveDateTime, NaiveDateTime), String> {
        if self.dt_from.is_none() {
            match parse_datetime(&self.string_value) {
                Ok((dt_from, dt_to)) => {
                    Ok((dt_from, dt_to))
                }
                _ => Err(String::from("Can't parse datetime: ") + &self.string_value),
            }
        } else {
            Ok((self.dt_from.unwrap(), self.dt_to.unwrap()))
        }
    }
}

impl Display for Variant {
    fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
        write!(f, "{}", self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn from_int() {
        let v = Variant::from_int(42);
        assert_eq!(v.to_string(), "42");
        assert_eq!(v.to_int(), 42);
        assert_eq!(v.to_float(), 42.0);
        assert!(matches!(v.get_type(), VariantType::Int));
    }

    #[test]
    fn from_int_negative() {
        let v = Variant::from_int(-7);
        assert_eq!(v.to_string(), "-7");
        assert_eq!(v.to_int(), -7);
        assert_eq!(v.to_float(), -7.0);
    }

    #[test]
    fn from_int_zero() {
        let v = Variant::from_int(0);
        assert_eq!(v.to_int(), 0);
        assert_eq!(v.to_float(), 0.0);
        assert_eq!(v.to_string(), "0");
    }

    #[test]
    fn from_float() {
        let v = Variant::from_float(3.14);
        assert_eq!(v.to_float(), 3.14);
        assert_eq!(v.to_int(), 3);
        assert_eq!(v.to_string(), "3.14");
        assert!(matches!(v.get_type(), VariantType::Float));
    }

    #[test]
    fn from_float_negative() {
        let v = Variant::from_float(-2.5);
        assert_eq!(v.to_float(), -2.5);
        assert_eq!(v.to_int(), -2);
    }

    #[test]
    fn from_float_whole_number() {
        let v = Variant::from_float(5.0);
        assert_eq!(v.to_float(), 5.0);
        assert_eq!(v.to_int(), 5);
    }

    #[test]
    fn from_string_plain() {
        let v = Variant::from_string(&String::from("hello"));
        assert_eq!(v.to_string(), "hello");
        assert!(matches!(v.get_type(), VariantType::String));
    }

    #[test]
    fn from_string_numeric() {
        let v = Variant::from_string(&String::from("123"));
        assert_eq!(v.to_string(), "123");
        assert_eq!(v.to_int(), 123);
        assert_eq!(v.to_float(), 123.0);
    }

    #[test]
    fn from_string_non_numeric_to_int() {
        let v = Variant::from_string(&String::from("abc"));
        assert_eq!(v.to_int(), 0);
    }

    #[test]
    fn from_string_non_numeric_to_float() {
        let v = Variant::from_string(&String::from("abc"));
        assert_eq!(v.to_float(), 0.0);
    }

    #[test]
    fn from_string_filesize_to_int() {
        let v = Variant::from_string(&String::from("1k"));
        assert_eq!(v.to_int(), 1024);
    }

    #[test]
    fn from_string_filesize_to_float() {
        let v = Variant::from_string(&String::from("1k"));
        assert_eq!(v.to_float(), 1024.0);
    }

    #[test]
    fn from_string_float_value() {
        let v = Variant::from_string(&String::from("3.14"));
        assert_eq!(v.to_float(), 3.14);
    }

    #[test]
    fn from_signed_string_positive() {
        let v = Variant::from_signed_string(&String::from("42"), false);
        assert_eq!(v.to_string(), "42");
        assert!(matches!(v.get_type(), VariantType::String));
    }

    #[test]
    fn from_signed_string_negative() {
        let v = Variant::from_signed_string(&String::from("42"), true);
        assert_eq!(v.to_string(), "-42");
    }

    #[test]
    fn from_bool_true() {
        let v = Variant::from_bool(true);
        assert_eq!(v.to_bool(), true);
        assert_eq!(v.to_string(), "true");
        assert_eq!(v.to_int(), 1);
        assert!(matches!(v.get_type(), VariantType::Bool));
    }

    #[test]
    fn from_bool_false() {
        let v = Variant::from_bool(false);
        assert_eq!(v.to_bool(), false);
        assert_eq!(v.to_string(), "false");
        assert_eq!(v.to_int(), 0);
    }

    #[test]
    fn from_datetime() {
        let dt = NaiveDate::from_ymd_opt(2024, 6, 15).unwrap().and_hms_opt(10, 30, 0).unwrap();
        let v = Variant::from_datetime(dt);
        assert_eq!(v.to_string(), "2024-06-15 10:30:00");
        assert!(matches!(v.get_type(), VariantType::DateTime));
        let (from, to) = v.to_datetime().unwrap();
        assert_eq!(from, dt);
        assert_eq!(to, dt);
    }

    #[test]
    fn to_datetime_from_string() {
        let v = Variant::from_string(&String::from("2024-06-15"));
        let result = v.to_datetime();
        assert!(result.is_ok());
    }

    #[test]
    fn to_datetime_from_invalid_string() {
        let v = Variant::from_string(&String::from("not-a-date"));
        let result = v.to_datetime();
        assert!(result.is_err());
    }

    #[test]
    fn to_bool_from_string_true_variants() {
        for val in &["true", "1", "yes", "y", "on"] {
            let v = Variant::from_string(&String::from(*val));
            assert_eq!(v.to_bool(), true, "expected true for '{}'", val);
        }
    }

    #[test]
    fn to_bool_from_string_false_variants() {
        for val in &["false", "0", "no", "n", "off"] {
            let v = Variant::from_string(&String::from(*val));
            assert_eq!(v.to_bool(), false, "expected false for '{}'", val);
        }
    }

    #[test]
    fn to_bool_from_string_unknown() {
        let v = Variant::from_string(&String::from("maybe"));
        assert_eq!(v.to_bool(), false);
    }

    #[test]
    fn to_bool_from_int_one() {
        let v = Variant::from_int(1);
        assert_eq!(v.to_bool(), true);
    }

    #[test]
    fn to_bool_from_int_zero() {
        let v = Variant::from_int(0);
        assert_eq!(v.to_bool(), false);
    }

    #[test]
    fn to_bool_from_int_other() {
        let v = Variant::from_int(42);
        assert_eq!(v.to_bool(), false);
    }

    #[test]
    fn to_bool_from_float_one() {
        let v = Variant::from_float(1.0);
        assert_eq!(v.to_bool(), true);
    }

    #[test]
    fn to_bool_from_float_zero() {
        let v = Variant::from_float(0.0);
        assert_eq!(v.to_bool(), false);
    }

    #[test]
    fn empty_variant() {
        let v = Variant::empty(VariantType::String);
        assert_eq!(v.to_string(), "");
        assert!(matches!(v.get_type(), VariantType::String));
    }

    #[test]
    fn empty_variant_to_int() {
        let v = Variant::empty(VariantType::Int);
        assert_eq!(v.to_int(), 0);
    }

    #[test]
    fn empty_variant_to_bool() {
        let v = Variant::empty(VariantType::Bool);
        assert_eq!(v.to_bool(), false);
    }

    #[test]
    fn display_trait() {
        let v = Variant::from_int(99);
        assert_eq!(format!("{}", v), "99");
    }

    #[test]
    fn display_trait_string() {
        let v = Variant::from_string(&String::from("test"));
        assert_eq!(format!("{}", v), "test");
    }

    #[test]
    fn clone() {
        let v1 = Variant::from_int(10);
        let v2 = v1.clone();
        assert_eq!(v2.to_int(), 10);
        assert_eq!(v2.to_string(), "10");
    }
}