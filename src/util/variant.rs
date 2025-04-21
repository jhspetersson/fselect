use std::fmt::{Display, Error, Formatter};

use chrono::NaiveDateTime;

use crate::util::{error_exit, format_datetime, parse_datetime, parse_filesize, str_to_bool};

#[derive(Clone, Debug)]
pub enum VariantType {
    String,
    Int,
    Float,
    Bool,
    DateTime,
}

#[derive(Debug)]
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
            str_to_bool(&self.string_value).expect("Can't parse boolean value")
        } else if let Some(int_value) = self.int_value {
            int_value == 1
        } else if let Some(float_value) = self.float_value {
            float_value == 1.0
        } else {
            false
        }
    }

    pub fn to_datetime(&self) -> (NaiveDateTime, NaiveDateTime) {
        if self.dt_from.is_none() {
            match parse_datetime(&self.string_value) {
                Ok((dt_from, dt_to)) => {
                    return (dt_from, dt_to);
                }
                _ => error_exit("Can't parse datetime", &self.string_value),
            }
        }

        (self.dt_from.unwrap(), self.dt_to.unwrap())
    }
}

impl Display for Variant {
    fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
        write!(f, "{}", self.to_string())
    }
}