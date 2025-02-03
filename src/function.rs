//! Functions for processing values in the query language.
//! This module contains both the regular and aggregate functions used in the query language.

use std::collections::HashMap;
use std::fmt::Display;
use std::fmt::Error;
use std::fmt::Formatter;
use std::fs::DirEntry;
use std::fs::File;
use std::io::prelude::*;
use std::str::FromStr;
use std::time::Duration;

use chrono::Datelike;
use chrono::Local;
use chrono::NaiveDateTime;
use human_time::ToHumanTimeString;
use rand::Rng;
use serde::ser::{Serialize, Serializer};
#[cfg(unix)]
use xattr::FileExt;

use crate::fileinfo::FileInfo;
use crate::util::{capitalize, error_exit, format_date, format_datetime};
use crate::util::{parse_filesize, parse_datetime, str_to_bool};

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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Hash)]
pub enum Function {
    // ===== Regular functions =====
    //  String conversion functions
    /// Convert the value to lowercase
    Lower,
    /// Convert the value to UPPERCASE
    Upper,
    /// Capitalize the first letter of each word (Title Case)
    InitCap,
    /// Get the length of the string
    Length,
    /// Convert the value to base64
    ToBase64,
    /// Read the value as base64
    FromBase64,

    //  String manipulation functions
    /// Concatenate the value with the arguments
    Concat,
    /// Concatenate the arguments, separated by the value
    ConcatWs,
    /// Get a substring of the value, from a position and length
    Substring,
    /// Replace a substring in the value with another string
    Replace,
    /// Trim whitespace from the value
    Trim,
    /// Trim whitespace from the start of the value
    LTrim,
    /// Trim whitespace from the end of the value
    RTrim,

    //  Numeric functions
    /// Get the binary representation of the value
    Bin,
    /// Get the hexadecimal representation of the value
    Hex,
    /// Get the octal representation of the value
    Oct,
    /// Get the absolute value of the number
    Abs,
    /// Raise the value to the power of another value
    Power,
    /// Get the square root of the value
    Sqrt,
    /// Get the logarithm of the value with a specific base
    Log,
    /// Get the natural logarithm of the value
    Ln,
    /// Get e raised to the power of the specified number
    Exp,

    //  Japanese string functions
    /// Check if the string contains Japanese characters
    ContainsJapanese,
    /// Check if the string contains Hiragana characters
    ContainsHiragana,
    /// Check if the string contains Katakana characters
    ContainsKatakana,
    /// Check if the string contains Kana characters
    ContainsKana,
    /// Check if the string contains Kanji characters
    ContainsKanji,

    //  Formatting functions
    /// Format a file size in human-readable format
    FormatSize,
    /// Format a time duration in human-readable format
    FormatTime,

    //  Date and time functions
    /// Get the current date
    CurrentDate,
    /// Get the day from a date
    Day,
    /// Get the month from a date
    Month,
    /// Get the year from a date
    Year,
    /// Get the day of the week from a date
    DayOfWeek,

    //  File functions
    #[cfg(all(unix, feature = "users"))]
    /// Get the current user ID
    CurrentUid,
    #[cfg(all(unix, feature = "users"))]
    /// Get the current username
    CurrentUser,
    #[cfg(all(unix, feature = "users"))]
    /// Get the current group ID
    CurrentGid,
    #[cfg(all(unix, feature = "users"))]
    /// Get the current group name
    CurrentGroup,

    /// Checks if a file contains a substring
    Contains,

    #[cfg(unix)]
    /// Check if the file has a specific extended attribute
    HasXattr,
    #[cfg(unix)]
    /// Get the value of an extended attribute
    Xattr,
    #[cfg(target_os = "linux")]
    /// Check if the file has capabilities (security.capability xattr)
    HasCapabilities,
    #[cfg(target_os = "linux")]
    /// Check if the file has a specific capability (security.capability xattr)
    HasCapability,

    //  Miscellaneous functions
    /// Return the first non-empty value
    Coalesce,
    /// Gets a random number from 0 to the value, or between two values
    Random,

    // ===== Aggregate functions =====
    /// Get the minimum value
    Min,
    /// Get the maximum value
    Max,
    /// Get the average value
    Avg,
    /// Get the sum of all values
    Sum,
    /// Get the number of values
    Count,

    /// Get the population standard deviation
    StdDevPop,
    /// Get the sample standard deviation
    StdDevSamp,
    /// Get the population variance
    VarPop,
    /// Get the sample variance
    VarSamp,
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
            "abs" => Ok(Function::Abs),
            "power" | "pow" => Ok(Function::Power),
            "sqrt" => Ok(Function::Sqrt),
            "log" => Ok(Function::Log),
            "ln" => Ok(Function::Ln),
            "exp" => Ok(Function::Exp),

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
            "format_time" | "pretty_time" => Ok(Function::FormatTime),

            "current_date" | "cur_date" | "curdate" => Ok(Function::CurrentDate),
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
            #[cfg(target_os = "linux")]
            "has_capabilities" | "has_caps" => Ok(Function::HasCapabilities),
            #[cfg(target_os = "linux")]
            "has_capability" | "has_cap" => Ok(Function::HasCapability),

            "rand" | "random" => Ok(Function::Random),

            _ => {
                let err = String::from("Unknown function ") + &function;
                Err(err)
            }
        }
    }
}

impl Display for Function {
    fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
        write!(f, "{:?}", self)
    }
}

impl Serialize for Function {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl Function {
    /// Check if the function is an aggregate function
    pub fn is_aggregate_function(&self) -> bool {
        matches!(
            self,
            Function::Min
                | Function::Max
                | Function::Avg
                | Function::Sum
                | Function::Count
                | Function::StdDevPop
                | Function::StdDevSamp
                | Function::VarPop
                | Function::VarSamp
        )
    }

    /// Check if the function is a numeric function, i.e. it returns a numeric value.
    pub fn is_numeric_function(&self) -> bool {
        if self.is_aggregate_function() {
            return true;
        }

        matches!(
            self,
            Function::Length
                | Function::Random
                | Function::Day
                | Function::Month
                | Function::Year
                | Function::Abs
                | Function::Power
                | Function::Sqrt
                | Function::Log
                | Function::Ln
                | Function::Exp
        )
    }

    /// Check if the function is a boolean function, i.e. it returns a boolean value.
    pub fn is_boolean_function(&self) -> bool {
        #[cfg(unix)]
        if self == &Function::HasXattr {
            return true;
        }

        #[cfg(target_os = "linux")]
        if self == &Function::HasCapabilities || self == &Function::HasCapability {
            return true;
        }

        matches!(
            self,
            Function::Contains
                | Function::ContainsHiragana
                | Function::ContainsKatakana
                | Function::ContainsKana
                | Function::ContainsKanji
                | Function::ContainsJapanese
        )
    }
}

/// Applies a function to a value and returns the result.
/// If no function is provided, the original value is returned.
///
/// Args:
///  function: Optional specification of which function to apply.
///  function_arg: The value to apply the function to.
///  function_args: Additional arguments to the function.
///  entry: Optional directory entry to read the file contents from.
///  file_info: Optional file information to read the file contents from.
///
/// Returns:
///   A variant containing the value computed or the original value if no function is provided.
pub fn get_value(
    function: &Option<Function>,
    function_arg: String,
    function_args: Vec<String>,
    entry: Option<&DirEntry>,
    file_info: &Option<FileInfo>,
) -> Variant {
    //* Refer to the Function enum for a list of available functions and their descriptions
    match function {
        // ===== String functions =====
        Some(Function::Lower) => Variant::from_string(&function_arg.to_lowercase()),
        Some(Function::Upper) => Variant::from_string(&function_arg.to_uppercase()),
        Some(Function::InitCap) => {
            let result = function_arg
                .split_whitespace()
                .map(|s| capitalize(&s.to_lowercase()))
                .collect::<Vec<_>>()
                .join(" ");
            Variant::from_string(&result)
        }
        // Get the length of the string
        Some(Function::Length) => {
            Variant::from_int(function_arg.chars().count() as i64)
        }
        // Convert the value to base64
        Some(Function::ToBase64) => {
            Variant::from_string(&rbase64::encode((function_arg).as_ref()))
        }
        // Read the value as base64
        Some(Function::FromBase64) => {
            Variant::from_string(
                &String::from_utf8_lossy(&rbase64::decode(&function_arg).unwrap_or_default())
                    .to_string(),
            )
        }

        // ===== String manipulation functions =====
        Some(Function::Concat) => {
            Variant::from_string(&(String::from(&function_arg) + &function_args.join("")))
        }
        Some(Function::ConcatWs) => Variant::from_string(&function_args.join(&function_arg)),
        Some(Function::Substring) => {
            let string = String::from(&function_arg);

            let mut pos: i32 = match &function_args.is_empty() {
                true => 0,
                false => *&function_args[0].parse::<i32>().unwrap() - 1,
            };

            if pos < 0 {
                let string_length = string.chars().count() as i32;
                pos = string_length - pos.abs() + 1;
            }

            let len = match &function_args.get(1) {
                Some(len) => len.parse::<usize>().unwrap(),
                _ => 0,
            };

            let result = match len > 0 {
                true => string.chars().skip(pos as usize).take(len).collect(),
                false => string.chars().skip(pos as usize).collect(),
            };

            Variant::from_string(&result)
        }
        Some(Function::Replace) => {
            let source = function_arg;
            let from = &function_args[0];
            let to = &function_args[1];

            let result = source.replace(from, to);

            Variant::from_string(&result)
        }
        Some(Function::Trim) => {
            Variant::from_string(&function_arg.trim().to_string())
        }
        Some(Function::LTrim) => {
            Variant::from_string(&function_arg.trim_start().to_string())
        }
        Some(Function::RTrim) => {
            Variant::from_string(&function_arg.trim_end().to_string())
        }

        // ===== Numeric functions =====
        Some(Function::Bin) => match function_arg.parse::<i64>() {
            Ok(val) => Variant::from_string(&format!("{:b}", val)),
            _ => Variant::empty(VariantType::String),
        },
        Some(Function::Hex) => match function_arg.parse::<i64>() {
            Ok(val) => Variant::from_string(&format!("{:x}", val)),
            _ => Variant::empty(VariantType::String),
        },
        Some(Function::Oct) => match function_arg.parse::<i64>() {
            Ok(val) => Variant::from_string(&format!("{:o}", val)),
            _ => Variant::empty(VariantType::String),
        },
        Some(Function::Abs) => match function_arg.parse::<f64>() {
            Ok(val) => Variant::from_float(val.abs()),
            _ => Variant::empty(VariantType::String),
        }
        Some(Function::Power) => {
            match function_arg.parse::<f64>() {
                Ok(val) => {
                    let power = match function_args.first() {
                        Some(power) => power.parse::<f64>().unwrap(),
                        _ => 0.0,
                    };

                    Variant::from_float(val.powf(power))
                }
                _ => Variant::empty(VariantType::String),
            }
        }
        Some(Function::Sqrt) => match function_arg.parse::<f64>() {
            Ok(val) => Variant::from_float(val.sqrt()),
            _ => Variant::empty(VariantType::String),
        },
        Some(Function::Log) => {
            match function_arg.parse::<f64>() {
                Ok(val) => {
                    let base = match function_args.first() {
                        Some(base) => base.parse::<f64>().unwrap(),
                        _ => 10.0,
                    };

                    Variant::from_float(val.log(base))
                }
                _ => Variant::empty(VariantType::String),
            }
        }
        Some(Function::Ln) => match function_arg.parse::<f64>() {
            Ok(val) => Variant::from_float(val.ln()),
            _ => Variant::empty(VariantType::String),
        }
        Some(Function::Exp) => match function_arg.parse::<f64>() {
            Ok(val) => Variant::from_float(val.exp()),
            _ => Variant::empty(VariantType::String),
        }

        // ===== Japanese string functions =====
        Some(Function::ContainsJapanese) => {
            Variant::from_bool(crate::util::japanese::contains_japanese(&function_arg))
        }
        Some(Function::ContainsHiragana) => {
            Variant::from_bool(crate::util::japanese::contains_hiragana(&function_arg))
        }
        Some(Function::ContainsKatakana) => {
            Variant::from_bool(crate::util::japanese::contains_katakana(&function_arg))
        }
        Some(Function::ContainsKana) => {
            Variant::from_bool(crate::util::japanese::contains_kana(&function_arg))
        }
        Some(Function::ContainsKanji) => {
            Variant::from_bool(crate::util::japanese::contains_kanji(&function_arg))
        }

        // ===== Formatting functions =====
        Some(Function::FormatSize) => {
            if function_arg.is_empty() {
                return Variant::empty(VariantType::String);
            }

            if let Ok(size) = function_arg.parse::<u64>() {
                let modifier = match function_args.first() {
                    Some(modifier) => modifier,
                    _ => "",
                };
                let file_size = crate::util::format_filesize(size, modifier);
                return Variant::from_string(&file_size);
            }

            Variant::empty(VariantType::String)
        }
        Some(Function::FormatTime) => {
            if function_arg.is_empty() {
                return Variant::empty(VariantType::String);
            }

            let seconds = function_arg.parse::<u64>().unwrap();
            let formatted = Duration::from_secs(seconds).to_human_time_string();
            Variant::from_string(&formatted)
        }

        // ===== Datetime functions =====
        Some(Function::CurrentDate) => {
            let now = Local::now().date_naive();
            Variant::from_string(&format_date(&now))
        }
        Some(Function::Year) => match parse_datetime(&function_arg) {
            Ok(date) => Variant::from_int(date.0.year() as i64),
            _ => Variant::empty(VariantType::Int),
        },
        Some(Function::Month) => match parse_datetime(&function_arg) {
            Ok(date) => Variant::from_int(date.0.month() as i64),
            _ => Variant::empty(VariantType::Int),
        },
        Some(Function::Day) => match parse_datetime(&function_arg) {
            Ok(date) => Variant::from_int(date.0.day() as i64),
            _ => Variant::empty(VariantType::Int),
        },
        Some(Function::DayOfWeek) => match parse_datetime(&function_arg) {
            Ok(date) => Variant::from_int(date.0.weekday().number_from_sunday() as i64),
            _ => Variant::empty(VariantType::Int),
        },

        // ===== File functions =====
        #[cfg(all(unix, feature = "users"))]
        Some(Function::CurrentUid) => Variant::from_int(uzers::get_current_uid() as i64),
        #[cfg(all(unix, feature = "users"))]
        Some(Function::CurrentUser) => {
            match uzers::get_current_username().and_then(|u| u.into_string().ok()) {
                Some(s) => Variant::from_string(&s),
                None => Variant::empty(VariantType::String),
            }
        }
        #[cfg(all(unix, feature = "users"))]
        Some(Function::CurrentGid) => Variant::from_int(uzers::get_current_gid() as i64),
        #[cfg(all(unix, feature = "users"))]
        Some(Function::CurrentGroup) => {
            match uzers::get_current_groupname().and_then(|u| u.into_string().ok()) {
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
                    if f.read_to_string(&mut contents).is_ok() {
                        if contents.contains(&function_arg) {
                            return Variant::from_bool(true);
                        } else {
                            return Variant::from_bool(false);
                        }
                    }
                }
            }

            Variant::empty(VariantType::Bool)
        }
        #[cfg(unix)]
        Some(Function::HasXattr) => {
            if let Some(entry) = entry {
                if let Ok(file) = File::open(entry.path()) {
                    if let Ok(xattr) = file.get_xattr(&function_arg) {
                        return Variant::from_bool(xattr.is_some());
                    }
                }
            }

            Variant::empty(VariantType::Bool)
        }
        #[cfg(unix)]
        Some(Function::Xattr) => {
            if let Some(entry) = entry {
                if let Ok(file) = File::open(entry.path()) {
                    if let Ok(Some(xattr)) = file.get_xattr(&function_arg) {
                        if let Ok(value) = String::from_utf8(xattr) {
                            return Variant::from_string(&value);
                        }
                    }
                }
            }

            Variant::empty(VariantType::String)
        }
        #[cfg(target_os = "linux")]
        Some(Function::HasCapabilities) => {
            if let Some(entry) = entry {
                if let Ok(file) = File::open(entry.path()) {
                    if let Ok(caps_xattr) = file.get_xattr("security.capability") {
                        return Variant::from_bool(caps_xattr.is_some());
                    }
                }
            }

            Variant::empty(VariantType::Bool)
        }
        #[cfg(target_os = "linux")]
        Some(Function::HasCapability) => {
            if let Some(entry) = entry {
                if let Ok(file) = File::open(entry.path()) {
                    if let Ok(Some(caps_xattr)) = file.get_xattr("security.capability") {
                        let caps_string = crate::util::capabilities::parse_capabilities(caps_xattr);
                        return Variant::from_bool(caps_string.contains(&function_arg));
                    }
                }
            }

            Variant::empty(VariantType::Bool)
        }
        // ===== Miscellaneous functions =====
        Some(Function::Coalesce) => {
            if !&function_arg.is_empty() {
                return Variant::from_string(&function_arg);
            }

            for arg in function_args {
                if !arg.is_empty() {
                    return Variant::from_string(&arg);
                }
            }

            Variant::empty(VariantType::String)
        }
        Some(Function::Random) => {
            let mut rng = rand::rng();

            if function_arg.is_empty() {
                return Variant::from_int(rng.random_range(0..i64::MAX));
            }

            match function_arg.parse::<i64>() {
                Ok(val) => {
                    if function_args.is_empty() {
                        Variant::from_int(rng.random_range(0..val))
                    } else {
                        let limit = function_args.first().unwrap();
                        match limit.parse::<i64>() {
                            Ok(limit) => Variant::from_int(rng.random_range(val..limit)),
                            _ => error_exit(
                                "Could not parse limit argument of RANDOM function",
                                limit.as_str(),
                            ),
                        }
                    }
                }
                _ => error_exit(
                    "Could not parse an argument of RANDOM function",
                    function_arg.as_str(),
                ),
            }
        }
        // If no function is specified, return the original value
        _ => Variant::empty(VariantType::String),
    }
}

/// Retrieves an aggregated value from a data buffer based on the specified function and key.
///
/// Args:
///   function: Optional specification of which aggregate function to apply.
///   raw_output_buffer: A vector of hashmaps, where each hashmap contains string key-value pairs.
///   buffer_key: The key to look up in each hashmap of the buffer.
///   default_value: An optional default value to return if the function is not specified.
///
/// Returns:
///   A string representation of the aggregate value computed or the default value if no function is provided.
pub fn get_aggregate_value(
    function: &Option<Function>,
    raw_output_buffer: &Vec<HashMap<String, String>>,
    buffer_key: String,
    default_value: &Option<String>,
) -> String {
    //* Refer to the Function enum for a list of available functions and their descriptions
    match function {
        Some(Function::Min) => {
            let min = raw_output_buffer
                .iter()
                .filter_map(|item| item.get(&buffer_key)) // Get the value from the buffer
                .filter_map(|value| value.parse::<i64>().ok()) // Parse the value and filter out errors
                .min()
                .unwrap_or(0); // If no items were found

            min.to_string()
        }
        Some(Function::Max) => {
            let max = raw_output_buffer
                .iter()
                .filter_map(|item| item.get(&buffer_key)) // Get the values from the buffer
                .filter_map(|value| value.parse::<i64>().ok()) // Parse the value and filter out errors
                .max()
                .unwrap_or(0); // If no items were found

            max.to_string()
        }
        Some(Function::Avg) => {
            if raw_output_buffer.is_empty() {
                return String::from("0");
            }

            get_mean(raw_output_buffer, &buffer_key).to_string()
        }
        Some(Function::Sum) => get_buffer_sum(raw_output_buffer, &buffer_key).to_string(),
        Some(Function::Count) => raw_output_buffer.len().to_string(),
        Some(Function::StdDevPop) => {
            if raw_output_buffer.is_empty() {
                return String::new();
            }

            let n = raw_output_buffer.len();
            let variance = get_variance(raw_output_buffer, &buffer_key, n);
            let result = variance.sqrt();

            result.to_string()
        }
        Some(Function::StdDevSamp) => {
            if raw_output_buffer.is_empty() {
                return String::new();
            }

            let size = raw_output_buffer.len();
            let n = if size == 1 { 1 } else { size - 1 };
            let variance = get_variance(raw_output_buffer, &buffer_key, n);
            let result = variance.sqrt();

            result.to_string()
        }
        Some(Function::VarPop) => {
            if raw_output_buffer.is_empty() {
                return String::new();
            }

            let n = raw_output_buffer.len();
            let variance = get_variance(raw_output_buffer, &buffer_key, n);

            variance.to_string()
        }
        Some(Function::VarSamp) => {
            if raw_output_buffer.is_empty() {
                return String::new();
            }

            let size = raw_output_buffer.len();
            let n = if size == 1 { 1 } else { size - 1 };
            let variance = get_variance(raw_output_buffer, &buffer_key, n);

            variance.to_string()
        }

        // If no function is specified, return the default value
        // If no default value was specified, return an empty string
        _ => match &default_value {
            Some(val) => val.to_owned(),
            _ => String::new(),
        },
    }
}

/// Get the variance of all values in the buffer, based on the buffer key.
/// If the value can't be parsed as usize, it will be ignored.
fn get_variance(
    raw_output_buffer: &Vec<HashMap<String, String>>,
    buffer_key: &String,
    n: usize,
) -> f64 {
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

/// Get the mean of all values in the buffer, based on the buffer key.
/// If the value can't be parsed as usize, it will be ignored.
fn get_mean(raw_output_buffer: &Vec<HashMap<String, String>>, buffer_key: &String) -> f64 {
    let sum = get_buffer_sum(raw_output_buffer, buffer_key);
    let size = raw_output_buffer.len();

    (sum / size) as f64
}

/// Get the sum of all values in the buffer, based on the buffer key.
/// If the value can't be parsed as usize, it will be ignored.
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
