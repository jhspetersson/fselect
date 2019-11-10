use std::collections::HashMap;
use std::fmt::Display;
use std::fmt::Formatter;
use std::fmt::Error;
use std::fs::DirEntry;
use std::fs::File;
use std::io::prelude::*;
use std::str::FromStr;

use chrono::Datelike;
use chrono::DateTime;
use chrono::Local;
use serde::ser::{Serialize, Serializer};
#[cfg(unix)]
use xattr::FileExt;

use crate::fileinfo::FileInfo;
use crate::util::format_datetime;
use crate::util::parse_datetime;
use crate::util::parse_filesize;
use crate::util::str_to_bool;

#[derive(Clone, Debug)]
pub enum VariantType {
    String,
    Int,
    Bool,
    DateTime,
}

#[derive(Debug)]
pub struct Variant {
    value_type: VariantType,
    empty: bool,
    string_value: String,
    int_value: Option<i64>,
    bool_value: bool,
    dt_from: Option<DateTime<Local>>,
    dt_to: Option<DateTime<Local>>,
}

impl Variant {
    pub fn empty(value_type: VariantType) -> Variant {
        Variant {
            value_type,
            empty: true,
            string_value: String::new(),
            int_value: None,
            bool_value: false,
            dt_from: None,
            dt_to: None,
        }
    }

    pub fn get_type(&self) -> VariantType {
        self.value_type.clone()
    }

    pub fn from_int(value: i64) -> Variant {
        Variant {
            value_type: VariantType::Int,
            empty: false,
            string_value: format!("{}", value),
            int_value: Some(value),
            bool_value: value == 1,
            dt_from: None,
            dt_to: None,
        }
    }

    pub fn from_string(value: &String) -> Variant {
        let bool_value = str_to_bool(&value);

        Variant {
            value_type: VariantType::String,
            empty: false,
            string_value: value.clone(),
            int_value: None,
            bool_value,
            dt_from: None,
            dt_to: None,
        }
    }

    pub fn from_bool(value: bool) -> Variant {
        Variant {
            value_type: VariantType::Bool,
            empty: false,
            string_value: match value { true => String::from("true"), _ => String::from("false") },
            int_value: match value { true => Some(1), _ => Some(0) },
            bool_value: value,
            dt_from: None,
            dt_to: None,
        }
    }

    pub fn from_datetime(value: DateTime<Local>) -> Variant {
        Variant {
            value_type: VariantType::DateTime,
            empty: false,
            string_value: format_datetime(&value),
            int_value: Some(0),
            bool_value: false,
            dt_from: Some(value),
            dt_to: Some(value),
        }
    }

    pub fn to_string(&self) -> String {
        self.string_value.clone()
    }

    pub fn to_int(&self) -> i64 {
        match self.int_value {
            Some(i) => i,
            None => {
                let int_value = self.string_value.parse::<usize>();
                match int_value {
                    Ok(i) => i as i64,
                    _ => match parse_filesize(&self.string_value) {
                        Some(size) => size as i64,
                        _ => 0
                    }
                }
            }
        }
    }

    pub fn to_bool(&self) -> bool {
        self.bool_value
    }

    pub fn to_datetime(&self) -> (DateTime<Local>, DateTime<Local>) {
        if self.dt_from.is_none() {
            match parse_datetime(&self.string_value) {
                Ok((dt_from, dt_to)) => {
                    return (dt_from, dt_to);
                },
                _ => panic!("Illegal datetime format")
            }
        }

        (self.dt_from.unwrap(), self.dt_to.unwrap())
    }
}

impl Display for Variant {
    fn fmt(&self, f: &mut Formatter) -> Result<(), Error>{
        write!(f, "{}", self.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Hash)]
pub enum Function {
    Lower,
    Upper,
    Length,
    Base64,
    Hex,
    Oct,

    ContainsJapanese,
    ContainsHiragana,
    ContainsKatakana,
    ContainsKana,
    ContainsKanji,

    Min,
    Max,
    Avg,
    Sum,
    Count,

    Day,
    Month,
    Year,

    Contains,

    HasXattr,
    Xattr,
}

impl FromStr for Function {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let function = s.to_ascii_lowercase();

        match function.as_str() {
            "lower" => Ok(Function::Lower),
            "upper" => Ok(Function::Upper),
            "length" => Ok(Function::Length),
            "base64" => Ok(Function::Base64),
            "hex" => Ok(Function::Hex),
            "oct" => Ok(Function::Oct),

            "contains_japanese" | "japanese" => Ok(Function::ContainsJapanese),
            "contains_hiragana" | "hiragana" => Ok(Function::ContainsHiragana),
            "contains_katakana" | "katakana" => Ok(Function::ContainsKatakana),
            "contains_kana" | "kana" => Ok(Function::ContainsKana),
            "contains_kanji" | "kanji" => Ok(Function::ContainsKanji),

            "day" => Ok(Function::Day),
            "month" => Ok(Function::Month),
            "year" => Ok(Function::Year),

            "min" => Ok(Function::Min),
            "max" => Ok(Function::Max),
            "avg" => Ok(Function::Avg),
            "sum" => Ok(Function::Sum),
            "count" => Ok(Function::Count),

            "contains" => Ok(Function::Contains),

            "has_xattr" => Ok(Function::HasXattr),
            "xattr" => Ok(Function::Xattr),

            _ => {
                let err = String::from("Unknown function ") + &function;
                Err(err)
            }
        }
    }
}

impl Display for Function {
    fn fmt(&self, f: &mut Formatter) -> Result<(), Error>{
        write!(f, "{:?}", self)
    }
}

impl Serialize for Function {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where S: Serializer
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl Function {
    pub fn is_aggregate_function(&self) -> bool {
        match self {
            Function::Min | Function::Max
            | Function::Avg | Function::Sum
            | Function::Count => true,
            _ => false
        }
    }

    pub fn is_numeric_function(&self) -> bool {
        if self.is_aggregate_function() {
            return true;
        }

        match self {
            Function::Length
            | Function::Day
            | Function::Month
            | Function::Year => true,
            _ => false
        }
    }
}

pub fn get_value(function: &Option<Function>,
                 function_arg: String,
                 entry: Option<&DirEntry>,
                 file_info: &Option<FileInfo>) -> Variant {
    match function {
        Some(Function::Lower) => {
            return Variant::from_string(&function_arg.to_lowercase());
        },
        Some(Function::Upper) => {
            return Variant::from_string(&function_arg.to_uppercase());
        },
        Some(Function::Length) => {
            return Variant::from_int(function_arg.chars().count() as i64);
        },
        Some(Function::Base64) => {
            return Variant::from_string(&base64::encode(&function_arg));
        },
        Some(Function::Hex) => {
            return match function_arg.parse::<i64>() {
                Ok(val) => Variant::from_string(&format!("{:x}", val)),
                _ => Variant::empty(VariantType::String)
            };
        },
        Some(Function::Oct) => {
            return match function_arg.parse::<i64>() {
                Ok(val) => Variant::from_string(&format!("{:o}", val)),
                _ => Variant::empty(VariantType::String)
            };
        },
        Some(Function::ContainsJapanese) => {
            let result = crate::util::japanese::contains_japanese(&function_arg);

            return Variant::from_bool(result);
        },
        Some(Function::ContainsHiragana) => {
            let result = crate::util::japanese::contains_hiragana(&function_arg);

            return Variant::from_bool(result);
        },
        Some(Function::ContainsKatakana) => {
            let result = crate::util::japanese::contains_katakana(&function_arg);

            return Variant::from_bool(result);
        },
        Some(Function::ContainsKana) => {
            let result = crate::util::japanese::contains_kana(&function_arg);

            return Variant::from_bool(result);
        },
        Some(Function::ContainsKanji) => {
            let result = crate::util::japanese::contains_kanji(&function_arg);

            return Variant::from_bool(result);
        },
        Some(Function::Year) => {
            match parse_datetime(&function_arg) {
                Ok(date) => {
                    return Variant::from_int(date.0.year() as i64);
                },
                _ => {
                    return Variant::empty(VariantType::Int);
                }
            }
        },
        Some(Function::Month) => {
            match parse_datetime(&function_arg) {
                Ok(date) => {
                    return Variant::from_int(date.0.month() as i64);
                },
                _ => {
                    return Variant::empty(VariantType::Int);
                }
            }
        },
        Some(Function::Day) => {
            match parse_datetime(&function_arg) {
                Ok(date) => {
                    return Variant::from_int(date.0.day() as i64);
                },
                _ => {
                    return Variant::empty(VariantType::Int);
                }
            }
        },
        Some(Function::Contains) => {
            if file_info.is_some() {
                return Variant::empty(VariantType::Bool);
            }

            if let Some(entry) = entry {
                if let Ok(mut f) = File::open(entry.path()) {
                    let mut contents = String::new();
                    if let Ok(_) = f.read_to_string(&mut contents) {
                        if contents.contains(&function_arg) {
                            return Variant::from_bool(true);
                        } else {
                            return Variant::from_bool(false);
                        }
                    }
                }
            }

            return Variant::empty(VariantType::Bool);
        },
        Some(Function::HasXattr) => {
            #[cfg(unix)]
                {
                    if let Some(entry) = entry {
                        if let Ok(file) = File::open(&entry.path()) {
                            if let Ok(xattr) = file.get_xattr(&function_arg) {
                                return Variant::from_bool(xattr.is_some());
                            }
                        }
                    }
                }

            return Variant::empty(VariantType::Bool);
        },
        Some(Function::Xattr) => {
            #[cfg(unix)]
                {
                    if let Some(entry) = entry {
                        if let Ok(file) = File::open(&entry.path()) {
                            if let Ok(xattr) = file.get_xattr(&function_arg) {
                                if let Some(xattr) = xattr {
                                    if let Ok(value) = String::from_utf8(xattr) {
                                        return Variant::from_string(&value);
                                    }
                                }
                            }
                        }
                    }
                }

            return Variant::empty(VariantType::String);
        },
        _ => {
            return Variant::empty(VariantType::String);
        }
    }
}

pub fn get_aggregate_value(function: &Option<Function>,
                           raw_output_buffer: &Vec<HashMap<String, String>>,
                           buffer_key: String,
                           default_value: &Option<String>) -> String {
    match function {
        Some(Function::Min) => {
            let mut min = -1;
            for value in raw_output_buffer {
                if let Some(value) = value.get(&buffer_key) {
                    if let Ok(value) = value.parse::<i64>() {
                        if min == -1 || value < min {
                            min = value;
                        }
                    }
                }
            }

            if min == -1 {
                min = 0;
            }

            return min.to_string();
        },
        Some(Function::Max) => {
            let mut max = 0;
            for value in raw_output_buffer {
                if let Some(value) = value.get(&buffer_key) {
                    if let Ok(value) = value.parse::<usize>() {
                        if value > max {
                            max = value;
                        }
                    }
                }
            }

            return max.to_string();
        },
        Some(Function::Avg) => {
            if raw_output_buffer.is_empty() {
                return String::from("0");
            }

            let mut sum = 0;
            for value in raw_output_buffer {
                if let Some(value) = value.get(&buffer_key) {
                    if let Ok(value) = value.parse::<usize>() {
                        sum += value;
                    }
                }
            }

            return (sum / raw_output_buffer.len()).to_string();
        },
        Some(Function::Sum) => {
            let mut sum = 0;
            for value in raw_output_buffer {
                if let Some(value) = value.get(&buffer_key) {
                    if let Ok(value) = value.parse::<usize>() {
                        sum += value;
                    }
                }
            }

            return sum.to_string();
        },
        Some(Function::Count) => {
            return raw_output_buffer.len().to_string();
        },
        _ => {
            match &default_value {
                Some(val) => return val.clone(),
                _ => return String::new()
            }
        }
    }
}
