use std::collections::HashMap;
use std::fmt::Display;
use std::fmt::Error;
use std::fmt::Formatter;
use std::fs::DirEntry;
use std::fs::File;
use std::io::prelude::*;
use std::str::FromStr;

use chrono::Datelike;
use chrono::DateTime;
use chrono::Local;
use rand::Rng;
use serde::ser::{Serialize, Serializer};
#[cfg(unix)]
use xattr::FileExt;

use crate::fileinfo::FileInfo;
use crate::util::{capitalize, format_date};
use crate::util::format_datetime;
use crate::util::parse_datetime;
use crate::util::parse_filesize;
use crate::util::str_to_bool;

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
    dt_from: Option<DateTime<Local>>,
    dt_to: Option<DateTime<Local>>,
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
            },
            false => value.to_owned()
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
            string_value: match value { true => String::from("true"), _ => String::from("false") },
            int_value: match value { true => Some(1), _ => Some(0) },
            float_value: None,
            bool_value: Some(value),
            dt_from: None,
            dt_to: None,
        }
    }

    pub fn from_datetime(value: DateTime<Local>) -> Variant {
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
                        _ => 0
                    }
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
                        _ => 0.0
                    }
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
    InitCap,
    Length,
    ToBase64,
    FromBase64,
    Bin,
    Hex,
    Oct,
    Power,
    Sqrt,

    ContainsJapanese,
    ContainsHiragana,
    ContainsKatakana,
    ContainsKana,
    ContainsKanji,

    Concat,
    ConcatWs,
    Substring,
    Replace,
    Trim,
    LTrim,
    RTrim,
    Coalesce,
    FormatSize,

    Min,
    Max,
    Avg,
    Sum,
    Count,

    StdDevPop,
    StdDevSamp,
    VarPop,
    VarSamp,

    CurrentDate,
    Day,
    Month,
    Year,
    DayOfWeek,

    #[cfg(all(unix, feature = "users"))]
    CurrentUid,
    #[cfg(all(unix, feature = "users"))]
    CurrentUser,
    #[cfg(all(unix, feature = "users"))]
    CurrentGid,
    #[cfg(all(unix, feature = "users"))]
    CurrentGroup,

    Contains,

    #[cfg(unix)]
    HasXattr,
    #[cfg(unix)]
    Xattr,

    Random,
}

impl FromStr for Function {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let function = s.to_ascii_lowercase();

        match function.as_str() {
            "lower" | "lowercase" | "lcase" => Ok(Function::Lower),
            "upper" | "uppercase" | "ucase" => Ok(Function::Upper),
            "length" | "len" => Ok(Function::Length),
            "initcap" => Ok(Function::InitCap),
            "to_base64" | "base64" => Ok(Function::ToBase64),
            "from_base64" => Ok(Function::FromBase64),
            "bin" => Ok(Function::Bin),
            "hex" => Ok(Function::Hex),
            "oct" => Ok(Function::Oct),
            "power" | "pow" => Ok(Function::Power),
            "sqrt" => Ok(Function::Sqrt),

            "contains_japanese" | "japanese" => Ok(Function::ContainsJapanese),
            "contains_hiragana" | "hiragana" => Ok(Function::ContainsHiragana),
            "contains_katakana" | "katakana" => Ok(Function::ContainsKatakana),
            "contains_kana" | "kana" => Ok(Function::ContainsKana),
            "contains_kanji" | "kanji" => Ok(Function::ContainsKanji),

            "concat" => Ok(Function::Concat),
            "concat_ws" => Ok(Function::ConcatWs),
            "substr" | "substring" => Ok(Function::Substring),
            "replace" => Ok(Function::Replace),
            "trim" => Ok(Function::Trim),
            "ltrim" => Ok(Function::LTrim),
            "rtrim" => Ok(Function::RTrim),
            "coalesce" => Ok(Function::Coalesce),
            "format_size" | "format_filesize" => Ok(Function::FormatSize),

            "current_date" | "curdate" => Ok(Function::CurrentDate),
            "day" => Ok(Function::Day),
            "month" => Ok(Function::Month),
            "year" => Ok(Function::Year),
            "dayofweek" | "dow" => Ok(Function::DayOfWeek),

            #[cfg(all(unix, feature = "users"))]
            "current_uid" => Ok(Function::CurrentUid),
            #[cfg(all(unix, feature = "users"))]
            "current_user" => Ok(Function::CurrentUser),
            #[cfg(all(unix, feature = "users"))]
            "current_gid" => Ok(Function::CurrentGid),
            #[cfg(all(unix, feature = "users"))]
            "current_group" => Ok(Function::CurrentGroup),

            "min" => Ok(Function::Min),
            "max" => Ok(Function::Max),
            "avg" => Ok(Function::Avg),
            "sum" => Ok(Function::Sum),
            "count" => Ok(Function::Count),

            "stddev_pop" | "stddev" | "std" => Ok(Function::StdDevPop),
            "stddev_samp" => Ok(Function::StdDevSamp),
            "var_pop" | "variance" => Ok(Function::VarPop),
            "var_samp" => Ok(Function::VarSamp),

            "contains" => Ok(Function::Contains),

            #[cfg(unix)]
            "has_xattr" => Ok(Function::HasXattr),
            #[cfg(unix)]
            "xattr" => Ok(Function::Xattr),

            "rand" | "random" => Ok(Function::Random),

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
            Function::Min
            | Function::Max
            | Function::Avg
            | Function::Sum
            | Function::Count
            | Function::StdDevPop
            | Function::StdDevSamp
            | Function::VarPop
            | Function::VarSamp => true,
            _ => false
        }
    }

    pub fn is_numeric_function(&self) -> bool {
        if self.is_aggregate_function() {
            return true;
        }

        match self {
            Function::Length
            | Function::Random
            | Function::Day
            | Function::Month
            | Function::Year
            | Function::Power
            | Function::Sqrt => true,
            _ => false
        }
    }

    pub fn is_boolean_function(&self) -> bool {
        #[cfg(unix)]
        if self == &Function::HasXattr {
            return true;
        }

        match self {
            Function::Contains
            | Function::ContainsHiragana
            | Function::ContainsKatakana
            | Function::ContainsKana
            | Function::ContainsKanji
            | Function::ContainsJapanese => true,
            _ => false
        }
    }
}

pub fn get_value(function: &Option<Function>,
                 function_arg: String,
                 function_args: Vec<String>,
                 entry: Option<&DirEntry>,
                 file_info: &Option<FileInfo>) -> Variant {
    match function {
        Some(Function::Lower) => {
            return Variant::from_string(&function_arg.to_lowercase());
        },
        Some(Function::Upper) => {
            return Variant::from_string(&function_arg.to_uppercase());
        },
        Some(Function::InitCap) => {
            let result = function_arg.split_whitespace().map(|s| capitalize(&s.to_lowercase())).collect::<Vec<_>>().join(" ");
            return Variant::from_string(&result);
        },
        Some(Function::Length) => {
            return Variant::from_int(function_arg.chars().count() as i64);
        },
        Some(Function::ToBase64) => {
            return Variant::from_string(&base64::encode(&function_arg));
        },
        Some(Function::FromBase64) => {
            return Variant::from_string(&String::from_utf8(base64::decode(&function_arg).unwrap_or(vec![])).unwrap_or(String::new()));
        },
        Some(Function::Bin) => {
            return match function_arg.parse::<i64>() {
                Ok(val) => Variant::from_string(&format!("{:b}", val)),
                _ => Variant::empty(VariantType::String)
            };
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
        Some(Function::Power) => {
            return match function_arg.parse::<f64>() {
                Ok(val) => {
                    let power = match function_args.get(0) {
                        Some(power) => power.parse::<f64>().unwrap(),
                        _ => 0.0
                    };

                    return Variant::from_float(val.powf(power));
                },
                _ => Variant::empty(VariantType::String)
            };
        },
        Some(Function::Sqrt) => {
            return match function_arg.parse::<f64>() {
                Ok(val) => {
                    return Variant::from_float(val.sqrt());
                },
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
        Some(Function::Concat) => {
            return Variant::from_string(&(String::from(&function_arg) + &function_args.join("")));
        },
        Some(Function::ConcatWs) => {
            return Variant::from_string(&function_args.join(&function_arg));
        },
        Some(Function::Substring) => {
            let string = String::from(&function_arg);

            let mut pos = match &function_args.is_empty() {
                true => 0,
                false => *&function_args[0].parse::<i32>().unwrap() - 1
            };

            if pos < 0 {
                let string_length = string.chars().count() as i32;
                pos = string_length - pos.abs() + 1;
            }

            let len = match &function_args.get(1) {
                Some(len) => len.parse::<usize>().unwrap(),
                _ => 0
            };

            let result = match len > 0 {
                true => string.chars().skip(pos as usize).take(len).collect(),
                false => string.chars().skip(pos as usize).collect()
            };

            return Variant::from_string(&result);
        },
        Some(Function::Replace) => {
            let source = function_arg;
            let from = &function_args[0];
            let to = &function_args[1];

            let result = source.replace(from, to);

            return Variant::from_string(&result);
        },
        Some(Function::Trim) => {
            return Variant::from_string(&function_arg.trim().to_string());
        },
        Some(Function::LTrim) => {
            return Variant::from_string(&function_arg.trim_start().to_string());
        },
        Some(Function::RTrim) => {
            return Variant::from_string(&function_arg.trim_end().to_string());
        },
        Some(Function::Coalesce) => {
            if !&function_arg.is_empty() {
                return Variant::from_string(&function_arg);
            }

            for arg in function_args {
                if !arg.is_empty() {
                    return Variant::from_string(&arg);
                }
            }

            return Variant::empty(VariantType::String);
        },
        Some(Function::FormatSize) => {
            if function_arg.is_empty() {
                return Variant::empty(VariantType::String);
            }

            if let Ok(size) = function_arg.parse::<u64>() {
                let modifier = match function_args.get(0) {
                    Some(modifier) => modifier,
                    _ => ""
                };
                let file_size = crate::util::format_filesize(size, modifier);
                return Variant::from_string(&file_size);
            }

            return Variant::empty(VariantType::String);
        }
        Some(Function::CurrentDate) => {
            let now = Local::today();
            return Variant::from_string(&format_date(&now));
        }
        Some(Function::Year) => match parse_datetime(&function_arg) {
            Ok(date) => {
                return Variant::from_int(date.0.year() as i64);
            }
            _ => {
                return Variant::empty(VariantType::Int);
            }
        },
        Some(Function::Month) => match parse_datetime(&function_arg) {
            Ok(date) => {
                return Variant::from_int(date.0.month() as i64);
            }
            _ => {
                return Variant::empty(VariantType::Int);
            }
        },
        Some(Function::Day) => match parse_datetime(&function_arg) {
            Ok(date) => {
                return Variant::from_int(date.0.day() as i64);
            }
            _ => {
                return Variant::empty(VariantType::Int);
            }
        },
        Some(Function::DayOfWeek) => match parse_datetime(&function_arg) {
            Ok(date) => {
                return Variant::from_int(date.0.weekday().number_from_sunday() as i64);
            }
            _ => {
                return Variant::empty(VariantType::Int);
            }
        },
        #[cfg(all(unix, feature = "users"))]
        Some(Function::CurrentUid) => {
            return Variant::from_int(users::get_current_uid() as i64);
        }
        #[cfg(all(unix, feature = "users"))]
        Some(Function::CurrentUser) => {
            match users::get_current_username().and_then(|u| u.into_string().ok()) {
                Some(s) => Variant::from_string(&s),
                None => Variant::empty(VariantType::String),
            }
        }
        #[cfg(all(unix, feature = "users"))]
        Some(Function::CurrentGid) => {
            return Variant::from_int(users::get_current_gid() as i64);
        }
        #[cfg(all(unix, feature = "users"))]
        Some(Function::CurrentGroup) => {
            match users::get_current_groupname().and_then(|u| u.into_string().ok()) {
                Some(s) => Variant::from_string(&s),
                None => Variant::empty(VariantType::String),
            }
        }
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
        }
        #[cfg(unix)]
        Some(Function::HasXattr) => {
            if let Some(entry) = entry {
                if let Ok(file) = File::open(&entry.path()) {
                    if let Ok(xattr) = file.get_xattr(&function_arg) {
                        return Variant::from_bool(xattr.is_some());
                    }
                }
            }

            return Variant::empty(VariantType::Bool);
        }
        #[cfg(unix)]
        Some(Function::Xattr) => {
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

            return Variant::empty(VariantType::String);
        }
        Some(Function::Random) => {
            let mut rng = rand::thread_rng();

            if function_arg.is_empty() {
                return Variant::from_int(rng.gen_range(0..i64::MAX));
            }

            match function_arg.parse::<i64>() {
                Ok(val) => {
                    if function_args.is_empty() {
                        return Variant::from_int(rng.gen_range(0..val));
                    } else {
                        let limit = function_args.get(0).unwrap();
                        match limit.parse::<i64>() {
                            Ok(limit) => return Variant::from_int(rng.gen_range(val..limit)),
                            _ => panic!("Could not parse function argument")
                        }
                    }
                },
                _ => panic!("Could not parse function argument")
            }
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

            get_mean(raw_output_buffer, &buffer_key).to_string()
        },
        Some(Function::Sum) => {
            get_buffer_sum(raw_output_buffer, &buffer_key).to_string()
        },
        Some(Function::Count) => {
            return raw_output_buffer.len().to_string();
        },
        Some(Function::StdDevPop) => {
            if raw_output_buffer.is_empty() {
                return String::new();
            }

            let n = raw_output_buffer.len();
            let variance = get_variance(raw_output_buffer, &buffer_key, n);
            let result = variance.sqrt();

            return result.to_string();
        },
        Some(Function::StdDevSamp) => {
            if raw_output_buffer.is_empty() {
                return String::new();
            }

            let size = raw_output_buffer.len();
            let n = if size == 1 { 1 } else { size - 1 };
            let variance = get_variance(raw_output_buffer, &buffer_key, n);
            let result = variance.sqrt();

            return result.to_string();
        },
        Some(Function::VarPop) => {
            if raw_output_buffer.is_empty() {
                return String::new();
            }

            let n = raw_output_buffer.len();
            let variance = get_variance(raw_output_buffer, &buffer_key, n);

            return variance.to_string();
        },
        Some(Function::VarSamp) => {
            if raw_output_buffer.is_empty() {
                return String::new();
            }

            let size = raw_output_buffer.len();
            let n = if size == 1 { 1 } else { size - 1 };
            let variance = get_variance(raw_output_buffer, &buffer_key, n);

            return variance.to_string();
        },

        _ => {
            match &default_value {
                Some(val) => return val.to_owned(),
                _ => return String::new()
            }
        }
    }
}

fn get_variance(raw_output_buffer: &Vec<HashMap<String, String>>,
                buffer_key: &String,
                n: usize) -> f64 {
    let avg = get_mean(raw_output_buffer, buffer_key);

    let mut result: f64 = 0.0;
    for value in raw_output_buffer {
        if let Some(value) = value.get(buffer_key) {
            if let Ok(value) = value.parse::<f64>() {
                result += (avg - value).powi(2) / n as f64;
            }
        }
    }

    result
}

fn get_mean(raw_output_buffer: &Vec<HashMap<String, String>>, buffer_key: &String) -> f64 {
    let sum = get_buffer_sum(raw_output_buffer, buffer_key);
    let size = raw_output_buffer.len();

    (sum / size) as f64
}

fn get_buffer_sum(raw_output_buffer: &Vec<HashMap<String, String>>, buffer_key: &String) -> usize {
    let mut sum = 0;
    for value in raw_output_buffer {
        if let Some(value) = value.get(buffer_key) {
            if let Ok(value) = value.parse::<usize>() {
                sum += value;
            }
        }
    }

    sum
}
