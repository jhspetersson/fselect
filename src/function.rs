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
use human_time::ToHumanTimeString;
use rand::RngExt;
use serde::ser::{Serialize, Serializer};
#[cfg(unix)]
use xattr::FileExt;

use crate::fileinfo::FileInfo;
use crate::util::{capitalize_initials, format_date, format_time, format_datetime, parse_datetime};
use crate::util::variant::{Variant, VariantType};

macro_rules! functions {
    (
        #[group_order = [$($group_order:literal),*]$(,)?]
        $(#[$enum_attrs:meta])*
        $vis:vis enum $enum_name:ident {
            $(
                #[text = [$($text:literal),*]$(,)? $(data_type = $data_type:literal)?]
                $(@is_aggregate = $is_aggregate:literal)?
                $(@weight = $weight:literal)?
                $(@group = $group:literal)?
                $(@description = $description:literal)?
                $(#[$variant_attrs:meta])*
                $variant:ident
            ),*
            $(,)?
        }
        
    ) => {
        $(#[$enum_attrs])*
        $vis enum $enum_name {
            $(
                $(#[$variant_attrs])*
                $variant,
            )*
        }

        impl FromStr for $enum_name {
            type Err = String;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let function = s.to_ascii_lowercase();

                match function.as_str() {
                    $(
                        $(#[$variant_attrs])*
                        $($text)|* => Ok($enum_name::$variant),
                    )*
                    _ => {
                        let err = String::from("Unknown function ") + &function;
                        Err(err)
                    }
                }
            }
        }
        
        impl Display for $enum_name {
            fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
                write!(f, "{:?}", self)
            }
        }

        impl Serialize for $enum_name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.to_string())
            }
        }
        
        impl $enum_name {
            pub fn is_numeric_function(&self) -> bool {
                match self {
                    $(
                        $(#[$variant_attrs])*
                        $enum_name::$variant => {
                            stringify!($($data_type)?) .replace("\"", "") == "numeric"
                        }
                    )*
                }
            }
            
            pub fn is_boolean_function(&self) -> bool {
                match self {
                    $(
                        $(#[$variant_attrs])*
                        $enum_name::$variant => {
                            stringify!($($data_type)?) .replace("\"", "") == "boolean"
                        }
                    )*
                }
            }
            
            pub fn is_aggregate_function(&self) -> bool {
                match self {
                    $(
                        $(#[$variant_attrs])*
                        $enum_name::$variant => {
                            stringify!($($is_aggregate)?) == "true"
                        }
                    )*
                }
            }
            
            pub fn get_weight(&self) -> i32 {
                match self {
                    $(
                        $(#[$variant_attrs])*
                        $enum_name::$variant => {
                            stringify!($($weight)?) .parse().unwrap_or(0)
                        }
                    )*
                }
            }
            
            pub fn get_groups() -> Vec<&'static str> {
                vec![
                    $($group_order),*
                ]
            }

            pub fn get_names_and_descriptions() -> HashMap<&'static str, Vec<(Vec<&'static str>, &'static str)>> {
                let mut map = HashMap::new();

                $(
                    $(#[$variant_attrs])*
                    {
                        if !map.contains_key($($group)?) {
                            map.insert($($group)?, vec![]);
                        }
                        let key = map.get_mut($($group)?).unwrap();
                        key.push((vec![$($text),*], $($description)?));
                    }
                )*

                map
            }
        }
    }
}

/// Applies a function to a value and returns the result.
/// If no function is provided, the original value is returned.
///
/// Args:
///  function: The specification of which function to apply.
///  function_arg: The value to apply the function to.
///  function_args: Additional arguments to the function.
///  entry: Optional directory entry to read the file contents from.
///  file_info: Optional file information to read the file contents from.
///
/// Returns:
pub fn get_value(
    function: &Function,
    function_arg: String,
    function_args: Vec<String>,
    entry: Option<&DirEntry>,
    file_info: &Option<FileInfo>,
) -> Result<Variant, String> {
    match function {
        // ===== String functions =====
        Function::Lower => Ok(Variant::from_string(&function_arg.to_lowercase())),
        Function::Upper => Ok(Variant::from_string(&function_arg.to_uppercase())),
        Function::InitCap => {
            let result = capitalize_initials(&function_arg);
            Ok(Variant::from_string(&result))
        }
        Function::Length => {
            Ok(Variant::from_int(function_arg.chars().count() as i64))
        }
        Function::ToBase64 => {
            Ok(Variant::from_string(&rbase64::encode((function_arg).as_ref())))
        }
        Function::FromBase64 => {
            Ok(Variant::from_string(
                &String::from_utf8_lossy(&rbase64::decode(&function_arg).unwrap_or_default())
                    .to_string(),
            ))
        }

        // ===== String manipulation functions =====
        Function::Concat => {
            Ok(Variant::from_string(&(String::from(&function_arg) + &function_args.join(""))))
        }
        Function::ConcatWs => Ok(Variant::from_string(&function_args.join(&function_arg))),
        Function::Locate => {
            if function_args.is_empty() {
                return Err("LOCATE requires a search substring argument".to_string());
            }
            let string = String::from(&function_arg);
            let substring = &function_args[0];
            let original_length = string.chars().count();
            let pos: i32 = match &function_args.get(1) {
                Some(pos) => match pos.parse::<i32>() {
                    Ok(p) => p.saturating_sub(1).max(0),
                    Err(_) => return Err(format!("Could not parse position argument of LOCATE function: {}", pos)),
                },
                _ => 0,
            };

            if pos as usize > original_length {
                return Ok(Variant::from_int(0));
            }

            let string: String = string.chars().skip(pos as usize).collect();

            let result = string
                .find(substring)
                .map(|byte_index| {
                    let char_index = string[..byte_index].chars().count();
                    char_index as i64 + pos as i64 + 1
                })
                .unwrap_or(0);

            Ok(Variant::from_int(result))
        },
        Function::Substring => {
            let string = String::from(&function_arg);

            let pos: i32 = match &function_args.is_empty() {
                true => 0,
                false => match function_args[0].parse::<i32>() {
                    Ok(p) if p < 0 => {
                        let string_length = string.chars().count() as i32;
                        (string_length + p).max(0)
                    }
                    Ok(p) => (p - 1).max(0),
                    Err(_) => return Err(format!("Could not parse position argument of SUBSTRING function: {}", function_args[0])),
                },
            };

            let len: Option<usize> = match &function_args.get(1) {
                Some(len) => match len.parse::<usize>() {
                    Ok(l) => Some(l),
                    Err(_) => return Err(format!("Could not parse length argument of SUBSTRING function: {}", len)),
                },
                _ => None,
            };

            let result: String = match len {
                Some(l) => string.chars().skip(pos as usize).take(l).collect(),
                None => string.chars().skip(pos as usize).collect(),
            };

            Ok(Variant::from_string(&result))
        }
        Function::Replace => {
            if function_args.len() < 2 {
                return Err("REPLACE requires two arguments: search string and replacement string".to_string());
            }
            let source = function_arg;
            let from = &function_args[0];
            let to = &function_args[1];

            if from.is_empty() {
                return Ok(Variant::from_string(&source));
            }

            let result = source.replace(from, to);

            Ok(Variant::from_string(&result))
        }
        Function::Trim => {
            Ok(Variant::from_string(&function_arg.trim().to_string()))
        }
        Function::LTrim => {
            Ok(Variant::from_string(&function_arg.trim_start().to_string()))
        }
        Function::RTrim => {
            Ok(Variant::from_string(&function_arg.trim_end().to_string()))
        }

        // ===== Numeric functions =====
        Function::Bin => match function_arg.parse::<i64>() {
            Ok(val) => Ok(Variant::from_string(&format!("{:b}", val))),
            _ => Ok(Variant::empty(VariantType::String)),
        },
        Function::Hex => match function_arg.parse::<i64>() {
            Ok(val) => Ok(Variant::from_string(&format!("{:x}", val))),
            _ => Ok(Variant::empty(VariantType::String)),
        },
        Function::Oct => match function_arg.parse::<i64>() {
            Ok(val) => Ok(Variant::from_string(&format!("{:o}", val))),
            _ => Ok(Variant::empty(VariantType::String)),
        },
        Function::Abs => match function_arg.parse::<f64>() {
            Ok(val) => Ok(Variant::from_float(val.abs())),
            _ => Ok(Variant::empty(VariantType::String)),
        }
        Function::Power => {
            match function_arg.parse::<f64>() {
                Ok(val) => {
                    let power = match function_args.first() {
                        Some(power) => match power.parse::<f64>() {
                            Ok(p) => p,
                            Err(_) => return Err(format!("Could not parse exponent argument of POWER function: {}", power)),
                        },
                        _ => 0.0,
                    };

                    let result = val.powf(power);
                    if result.is_nan() || result.is_infinite() {
                        return Err(format!("POWER({}, {}) produces a non-finite result", val, power));
                    }
                    Ok(Variant::from_float(result))
                }
                _ => Ok(Variant::empty(VariantType::String)),
            }
        }
        Function::Sqrt => match function_arg.parse::<f64>() {
            Ok(val) if val < 0.0 => Err(format!("SQRT of a negative number: {}", val)),
            Ok(val) => Ok(Variant::from_float(val.sqrt())),
            _ => Ok(Variant::empty(VariantType::String)),
        },
        Function::Log => {
            match function_arg.parse::<f64>() {
                Ok(val) => {
                    let base = match function_args.first() {
                        Some(base) => match base.parse::<f64>() {
                            Ok(b) => b,
                            Err(_) => return Err(format!("Could not parse base argument of LOG function: {}", base)),
                        },
                        _ => 10.0,
                    };

                    if val <= 0.0 {
                        return Err(format!("LOG of a non-positive number: {}", val));
                    }
                    if base <= 0.0 || base == 1.0 {
                        return Err(format!("LOG with invalid base: {}", base));
                    }

                    Ok(Variant::from_float(val.log(base)))
                }
                _ => Ok(Variant::empty(VariantType::String)),
            }
        }
        Function::Ln => match function_arg.parse::<f64>() {
            Ok(val) if val <= 0.0 => Err(format!("LN of a non-positive number: {}", val)),
            Ok(val) => Ok(Variant::from_float(val.ln())),
            _ => Ok(Variant::empty(VariantType::String)),
        }
        Function::Exp => match function_arg.parse::<f64>() {
            Ok(val) => {
                let result = val.exp();
                if result.is_infinite() {
                    return Err(format!("EXP({}) overflows to infinity", val));
                }
                Ok(Variant::from_float(result))
            }
            _ => Ok(Variant::empty(VariantType::String)),
        }
        Function::Least => {
            match function_arg.parse::<f64>() {
                Ok(val) => {
                    let mut least = if val.is_finite() { val } else { f64::INFINITY };
                    for arg in function_args {
                        if let Ok(val) = arg.parse::<f64>() {
                            if val.is_finite() {
                                least = least.min(val);
                            }
                        }
                    }

                    if least.is_finite() {
                        Ok(Variant::from_float(least))
                    } else {
                        Ok(Variant::empty(VariantType::String))
                    }
                }
                _ => Ok(Variant::empty(VariantType::String)),
            }
        }
        Function::Greatest => {
            match function_arg.parse::<f64>() {
                Ok(val) => {
                    let mut greatest = if val.is_finite() { val } else { f64::NEG_INFINITY };
                    for arg in function_args {
                        if let Ok(val) = arg.parse::<f64>() {
                            if val.is_finite() {
                                greatest = greatest.max(val);
                            }
                        }
                    }

                    if greatest.is_finite() {
                        Ok(Variant::from_float(greatest))
                    } else {
                        Ok(Variant::empty(VariantType::String))
                    }
                }
                _ => Ok(Variant::empty(VariantType::String)),
            }
        }
        Function::Pi => {
            Ok(Variant::from_float(std::f64::consts::PI))
        }
        Function::Floor => match function_arg.parse::<f64>() {
            Ok(val) => Ok(Variant::from_float(val.floor())),
            _ => Ok(Variant::empty(VariantType::String)),
        },
        Function::Ceil => match function_arg.parse::<f64>() {
            Ok(val) => Ok(Variant::from_float(val.ceil())),
            _ => Ok(Variant::empty(VariantType::String)),
        },
        Function::Round => match function_arg.parse::<f64>() {
            Ok(val) => {
                let precision: i32 = match function_args.first() {
                    Some(p) => match p.parse::<i32>() {
                        Ok(p) => p,
                        Err(_) => return Err(format!("Could not parse precision argument of ROUND function: {}", p)),
                    },
                    _ => 0,
                };
                let factor = 10_f64.powi(precision);
                let result = (val * factor).round() / factor;
                if result.is_finite() {
                    Ok(Variant::from_float(result))
                } else if precision >= 0 {
                    Ok(Variant::from_float(val))
                } else {
                    Ok(Variant::from_float(0.0))
                }
            }
            _ => Ok(Variant::empty(VariantType::String)),
        },

        // ===== Japanese string functions =====
        Function::ContainsJapanese => {
            Ok(Variant::from_bool(crate::util::japanese::contains_japanese(&function_arg)))
        }
        Function::ContainsHiragana => {
            Ok(Variant::from_bool(crate::util::japanese::contains_hiragana(&function_arg)))
        }
        Function::ContainsKatakana => {
            Ok(Variant::from_bool(crate::util::japanese::contains_katakana(&function_arg)))
        }
        Function::ContainsKana => {
            Ok(Variant::from_bool(crate::util::japanese::contains_kana(&function_arg)))
        }
        Function::ContainsKanji => {
            Ok(Variant::from_bool(crate::util::japanese::contains_kanji(&function_arg)))
        }

        // ===== Greek string functions =====
        Function::ContainsGreek => {
            Ok(Variant::from_bool(crate::util::greek::contains_greek(&function_arg)))
        }

        // ===== Formatting functions =====
        Function::FormatSize => {
            if function_arg.is_empty() {
                return Ok(Variant::empty(VariantType::String));
            }

            if let Ok(size) = function_arg.parse::<u64>() {
                let modifier = match function_args.first() {
                    Some(modifier) => modifier,
                    _ => "",
                };
                let file_size = crate::util::format_filesize(size, modifier).unwrap_or(String::new());
                return Ok(Variant::from_string(&file_size));
            }

            Ok(Variant::empty(VariantType::String))
        }
        Function::FormatTime => {
            if function_arg.is_empty() {
                return Ok(Variant::empty(VariantType::String));
            }

            if let Ok(seconds) = function_arg.parse::<u64>() {
                let formatted = Duration::from_secs(seconds).to_human_time_string();
                return Ok(Variant::from_string(&formatted));
            }

            Ok(Variant::empty(VariantType::String))
        }

        // ===== Datetime functions =====
        Function::CurrentDate => {
            let now = Local::now().date_naive();
            Ok(Variant::from_string(&format_date(&now)))
        }
        Function::CurrentTime => {
            let now = Local::now().time();
            Ok(Variant::from_string(&format_time(&now)))
        }
        Function::CurrentTimestamp => {
            let now = Local::now().naive_local();
            Ok(Variant::from_string(&format_datetime(&now)))
        }
        Function::Year => match parse_datetime(&function_arg) {
            Ok(date) => Ok(Variant::from_int(date.0.year() as i64)),
            _ => Ok(Variant::empty(VariantType::Int)),
        },
        Function::Month => match parse_datetime(&function_arg) {
            Ok(date) => Ok(Variant::from_int(date.0.month() as i64)),
            _ => Ok(Variant::empty(VariantType::Int)),
        },
        Function::Day => match parse_datetime(&function_arg) {
            Ok(date) => Ok(Variant::from_int(date.0.day() as i64)),
            _ => Ok(Variant::empty(VariantType::Int)),
        },
        Function::DayOfWeek => match parse_datetime(&function_arg) {
            Ok(date) => Ok(Variant::from_int(date.0.weekday().number_from_sunday() as i64)),
            _ => Ok(Variant::empty(VariantType::Int)),
        },

        #[cfg(all(unix, feature = "users"))]
        Function::CurrentUid => Ok(Variant::from_int(uzers::get_current_uid() as i64)),
        #[cfg(all(unix, feature = "users"))]
        Function::CurrentUser => {
            match uzers::get_current_username().and_then(|u| u.into_string().ok()) {
                Some(s) => Ok(Variant::from_string(&s)),
                None => Ok(Variant::empty(VariantType::String)),
            }
        }
        #[cfg(all(unix, feature = "users"))]
        Function::CurrentGid => Ok(Variant::from_int(uzers::get_current_gid() as i64)),
        #[cfg(all(unix, feature = "users"))]
        Function::CurrentGroup => {
            match uzers::get_current_groupname().and_then(|u| u.into_string().ok()) {
                Some(s) => Ok(Variant::from_string(&s)),
                None => Ok(Variant::empty(VariantType::String)),
            }
        }
        // ===== File functions =====
        Function::Contains => {
            if file_info.is_some() {
                return Ok(Variant::empty(VariantType::Bool));
            }

            if let Some(entry) = entry {
                if let Ok(mut f) = File::open(entry.path()) {
                    let mut contents = String::new();
                    if f.read_to_string(&mut contents).is_ok() {
                        if contents.contains(&function_arg) {
                            return Ok(Variant::from_bool(true));
                        } else {
                            return Ok(Variant::from_bool(false));
                        }
                    }
                }
            }

            Ok(Variant::empty(VariantType::Bool))
        }
        #[cfg(unix)]
        Function::HasXattr => {
            if let Some(entry) = entry {
                if let Ok(file) = File::open(entry.path()) {
                    if let Ok(xattr) = file.get_xattr(&function_arg) {
                        return Ok(Variant::from_bool(xattr.is_some()));
                    }
                }
            }

            Ok(Variant::empty(VariantType::Bool))
        }
        #[cfg(unix)]
        Function::Xattr => {
            if let Some(entry) = entry {
                if let Ok(file) = File::open(entry.path()) {
                    if let Ok(Some(xattr)) = file.get_xattr(&function_arg) {
                        if let Ok(value) = String::from_utf8(xattr) {
                            return Ok(Variant::from_string(&value));
                        }
                    }
                }
            }

            Ok(Variant::empty(VariantType::String))
        }
        #[cfg(target_os = "linux")]
        Function::HasExtattr => {
            if let Some(entry) = entry {
                if let Ok(file) = File::open(entry.path()) {
                    if let Some(flags) = crate::util::extattrs::get_ext_attrs(&file) {
                        return Ok(Variant::from_bool(
                            crate::util::extattrs::has_ext_attr(flags, &function_arg),
                        ));
                    }
                }
            }

            Ok(Variant::empty(VariantType::Bool))
        }
        #[cfg(target_os = "linux")]
        Function::Acl => {
            if let Some(entry) = entry {
                if let Ok(file) = File::open(entry.path()) {
                    if let Ok(Some(acl_data)) = file.get_xattr("system.posix_acl_access") {
                        if let Some(entries) = crate::util::acl::parse_acl(&acl_data) {
                            return Ok(Variant::from_string(&crate::util::acl::format_acl(&entries)));
                        }
                    }
                }
            }

            Ok(Variant::empty(VariantType::String))
        }
        #[cfg(target_os = "linux")]
        Function::HasAclEntry => {
            if let Some(entry) = entry {
                if let Ok(file) = File::open(entry.path()) {
                    if let Ok(Some(acl_data)) = file.get_xattr("system.posix_acl_access") {
                        if let Some(entries) = crate::util::acl::parse_acl(&acl_data) {
                            return Ok(Variant::from_bool(
                                crate::util::acl::find_entry(&entries, &function_arg).is_some(),
                            ));
                        }
                    }
                }
            }

            Ok(Variant::empty(VariantType::Bool))
        }
        #[cfg(target_os = "linux")]
        Function::AclEntry => {
            if let Some(entry) = entry {
                if let Ok(file) = File::open(entry.path()) {
                    if let Ok(Some(acl_data)) = file.get_xattr("system.posix_acl_access") {
                        if let Some(entries) = crate::util::acl::parse_acl(&acl_data) {
                            if let Some(acl_entry) = crate::util::acl::find_entry(&entries, &function_arg) {
                                return Ok(Variant::from_string(&crate::util::acl::format_entry(acl_entry)));
                            }
                        }
                    }
                }
            }

            Ok(Variant::empty(VariantType::String))
        }
        #[cfg(target_os = "linux")]
        Function::DefaultAcl => {
            if let Some(entry) = entry {
                if entry.path().is_dir() {
                    if let Ok(file) = File::open(entry.path()) {
                        if let Ok(Some(acl_data)) = file.get_xattr("system.posix_acl_default") {
                            if let Some(entries) = crate::util::acl::parse_acl(&acl_data) {
                                return Ok(Variant::from_string(&crate::util::acl::format_acl(&entries)));
                            }
                        }
                    }
                }
            }

            Ok(Variant::empty(VariantType::String))
        }
        #[cfg(target_os = "linux")]
        Function::HasDefaultAclEntry => {
            if let Some(entry) = entry {
                if entry.path().is_dir() {
                    if let Ok(file) = File::open(entry.path()) {
                        if let Ok(Some(acl_data)) = file.get_xattr("system.posix_acl_default") {
                            if let Some(entries) = crate::util::acl::parse_acl(&acl_data) {
                                return Ok(Variant::from_bool(
                                    crate::util::acl::find_entry(&entries, &function_arg).is_some(),
                                ));
                            }
                        }
                    }
                }
            }

            Ok(Variant::empty(VariantType::Bool))
        }
        #[cfg(target_os = "linux")]
        Function::DefaultAclEntry => {
            if let Some(entry) = entry {
                if entry.path().is_dir() {
                    if let Ok(file) = File::open(entry.path()) {
                        if let Ok(Some(acl_data)) = file.get_xattr("system.posix_acl_default") {
                            if let Some(entries) = crate::util::acl::parse_acl(&acl_data) {
                                if let Some(acl_entry) = crate::util::acl::find_entry(&entries, &function_arg) {
                                    return Ok(Variant::from_string(&crate::util::acl::format_entry(acl_entry)));
                                }
                            }
                        }
                    }
                }
            }

            Ok(Variant::empty(VariantType::String))
        }
        #[cfg(target_os = "linux")]
        Function::HasCapabilities => {
            if let Some(entry) = entry {
                if let Ok(file) = File::open(entry.path()) {
                    if let Ok(caps_xattr) = file.get_xattr("security.capability") {
                        return Ok(Variant::from_bool(caps_xattr.is_some()));
                    }
                }
            }

            Ok(Variant::empty(VariantType::Bool))
        }
        #[cfg(target_os = "linux")]
        Function::HasCapability => {
            if let Some(entry) = entry {
                if let Ok(file) = File::open(entry.path()) {
                    if let Ok(Some(caps_xattr)) = file.get_xattr("security.capability") {
                        let caps_string = crate::util::capabilities::parse_capabilities(caps_xattr);
                        return Ok(Variant::from_bool(caps_string.contains(&function_arg)));
                    }
                }
            }

            Ok(Variant::empty(VariantType::Bool))
        }
        // ===== Miscellaneous functions =====
        Function::Coalesce => {
            if !&function_arg.is_empty() {
                return Ok(Variant::from_string(&function_arg));
            }

            for arg in function_args {
                if !arg.is_empty() {
                    return Ok(Variant::from_string(&arg));
                }
            }

            Ok(Variant::empty(VariantType::String))
        }
        Function::Random => {
            let mut rng = rand::rng();

            if function_arg.is_empty() {
                return Ok(Variant::from_int(rng.random_range(0..i64::MAX)));
            }

            match function_arg.parse::<i64>() {
                Ok(val) => {
                    if function_args.is_empty() {
                        if val <= 0 {
                            Ok(Variant::from_int(0))
                        } else {
                            Ok(Variant::from_int(rng.random_range(0..=val)))
                        }
                    } else {
                        let limit = function_args.first().unwrap();
                        match limit.parse::<i64>() {
                            Ok(limit) => {
                                if val >= limit {
                                    Ok(Variant::from_int(val))
                                } else {
                                    Ok(Variant::from_int(rng.random_range(val..=limit)))
                                }
                            }
                            _ => Err(format!(
                                "Could not parse limit argument of RANDOM function: {}",
                                limit
                            )),
                        }
                    }
                }
                _ => Err(format!(
                    "Could not parse an argument of RANDOM function: {}",
                    function_arg
                )),
            }
        }
        _ => Ok(Variant::empty(VariantType::String)),
    }
}

pub fn get_aggregate_value(
    function: &Function,
    accumulator: &GroupAccumulator,
    buffer_key: String,
    default_value: &Option<String>,
) -> String {
    let field_acc = accumulator.fields.get(&buffer_key);
    match function {
        Function::Min => {
            match field_acc {
                Some(acc) if acc.count > 0 => (acc.min + 0.0).to_string(),
                _ => String::new(),
            }
        }
        Function::Max => {
            match field_acc {
                Some(acc) if acc.count > 0 => (acc.max + 0.0).to_string(),
                _ => String::new(),
            }
        }
        Function::Avg => {
            match field_acc {
                Some(acc) if acc.count > 0 => {
                    let mean = acc.sum / acc.count as f64;
                    if mean.is_finite() { mean.to_string() } else { String::new() }
                }
                _ => String::new(),
            }
        }
        Function::Sum => {
            match field_acc {
                Some(acc) if acc.count > 0 => {
                    if acc.sum.is_finite() { acc.sum.to_string() } else { String::new() }
                }
                _ => String::new(),
            }
        }
        Function::Count => accumulator.total_count.to_string(),
        Function::StdDevPop => {
            match field_acc {
                Some(acc) if acc.count > 0 => {
                    let variance = acc.m2 / acc.count as f64;
                    if variance.is_finite() { variance.sqrt().to_string() } else { String::new() }
                }
                _ => String::new(),
            }
        }
        Function::StdDevSamp => {
            match field_acc {
                Some(acc) if acc.count > 1 => {
                    let variance = acc.m2 / (acc.count - 1) as f64;
                    if variance.is_finite() { variance.sqrt().to_string() } else { String::new() }
                }
                _ => String::new(),
            }
        }
        Function::VarPop => {
            match field_acc {
                Some(acc) if acc.count > 0 => {
                    let variance = acc.m2 / acc.count as f64;
                    if variance.is_finite() { variance.to_string() } else { String::new() }
                }
                _ => String::new(),
            }
        }
        Function::VarSamp => {
            match field_acc {
                Some(acc) if acc.count > 1 => {
                    let variance = acc.m2 / (acc.count - 1) as f64;
                    if variance.is_finite() { variance.to_string() } else { String::new() }
                }
                _ => String::new(),
            }
        }
        _ => match &default_value {
            Some(val) => val.to_owned(),
            _ => String::new(),
        },
    }
}

functions! {
    #[group_order = ["String", "Japanese string", "Numeric", "Datetime", "Aggregate", "Xattr", "Other"]]
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Hash)]
    pub enum Function {
        #[text = ["lower", "lowercase", "lcase"]]
        @group = "String"
        @description = "Convert the value to lowercase"
        Lower,

        #[text = ["upper", "uppercase", "ucase"]]
        @group = "String"
        @description = "Convert the value to UPPERCASE"
        Upper,

        #[text = ["initcap"]]
        @group = "String"
        @description = "Capitalize the first letter of each word (Title Case)"
        InitCap,

        #[text = ["length", "len"], data_type = "numeric"]
        @group = "String"
        @description = "Get the length of the string"
        Length,

        #[text = ["to_base64", "base64"]]
        @group = "String"
        @description = "Convert the value to base64"
        ToBase64,

        #[text = ["from_base64"]]
        @group = "String"
        @description = "Read the value as base64"
        FromBase64,

        #[text = ["concat"]]
        @group = "String"
        @description = "Concatenate the value with the arguments"
        Concat,

        #[text = ["concat_ws"]]
        @group = "String"
        @description = "Concatenate the arguments, separated by the value"
        ConcatWs,

        #[text = ["locate", "position"], data_type = "numeric"]
        @group = "String"
        @description = "Get the position of a substring in the value"
        Locate,

        #[text = ["substr", "substring"]]
        @group = "String"
        @description = "Get a substring of the value, from a position and length"
        Substring,

        #[text = ["replace"]]
        @group = "String"
        @description = "Replace a substring in the value with another string"
        Replace,

        #[text = ["trim"]]
        @group = "String"
        @description = "Trim whitespace from the value"
        Trim,

        #[text = ["ltrim"]]
        @group = "String"
        @description = "Trim whitespace from the start of the value"
        LTrim,

        #[text = ["rtrim"]]
        @group = "String"
        @description = "Trim whitespace from the end of the value"
        RTrim,

        #[text = ["bin"]]
        @group = "Numeric"
        @description = "Get the binary representation of the value"
        Bin,

        #[text = ["hex"]]
        @group = "Numeric"
        @description = "Get the hexadecimal representation of the value"
        Hex,

        #[text = ["oct"]]
        @group = "Numeric"
        @description = "Get the octal representation of the value"
        Oct,

        #[text = ["abs"], data_type = "numeric"]
        @group = "Numeric"
        @description = "Get the absolute value of the number"
        Abs,

        #[text = ["power", "pow"], data_type = "numeric"]
        @group = "Numeric"
        @description = "Raise the value to the power of another value"
        Power,

        #[text = ["sqrt"], data_type = "numeric"]
        @group = "Numeric"
        @description = "Get the square root of the value"
        Sqrt,

        #[text = ["log"], data_type = "numeric"]
        @group = "Numeric"
        @description = "Get the logarithm of the value with a specific base"
        Log,

        #[text = ["ln"], data_type = "numeric"]
        @group = "Numeric"
        @description = "Get the natural logarithm of the value"
        Ln,

        #[text = ["exp"], data_type = "numeric"]
        @group = "Numeric"
        @description = "Get e raised to the power of the specified number"
        Exp,

        #[text = ["least"], data_type = "numeric"]
        @group = "Numeric"
        @description = "Get the smallest value"
        Least,

        #[text = ["greatest"], data_type = "numeric"]
        @group = "Numeric"
        @description = "Get the largest value"
        Greatest,

        #[text = ["pi"], data_type = "numeric"]
        @weight = 1
        @group = "Numeric"
        @description = "Get the value of Pi (π)"
        Pi,

        #[text = ["floor"], data_type = "numeric"]
        @group = "Numeric"
        @description = "Round down to the nearest integer"
        Floor,

        #[text = ["ceil", "ceiling"], data_type = "numeric"]
        @group = "Numeric"
        @description = "Round up to the nearest integer"
        Ceil,

        #[text = ["round"], data_type = "numeric"]
        @group = "Numeric"
        @description = "Round to the nearest integer, or to a given number of decimal places"
        Round,

        #[text = ["contains_japanese", "japanese"], data_type = "boolean"]
        @group = "Japanese string"
        @description = "Check if the string contains Japanese characters"
        ContainsJapanese,

        #[text = ["contains_hiragana", "hiragana"], data_type = "boolean"]
        @group = "Japanese string"
        @description = "Check if the string contains Hiragana characters"
        ContainsHiragana,

        #[text = ["contains_katakana", "katakana"], data_type = "boolean"]
        @group = "Japanese string"
        @description = "Check if the string contains Katakana characters"
        ContainsKatakana,

        #[text = ["contains_kana", "kana"], data_type = "boolean"]
        @group = "Japanese string"
        @description = "Check if the string contains Kana characters"
        ContainsKana,

        #[text = ["contains_kanji", "kanji"], data_type = "boolean"]
        @group = "Japanese string"
        @description = "Check if the string contains Kanji characters"
        ContainsKanji,

        #[text = ["contains_greek", "greek"], data_type = "boolean"]
        @group = "Greek string"
        @description = "Check if the string contains Greek characters"
        ContainsGreek,

        #[text = ["format_size", "format_filesize"]]
        @group = "Other"
        @description = "Format a file size in human-readable format"
        FormatSize,

        #[text = ["format_time", "pretty_time"]]
        @group = "Other"
        @description = "Format a time duration in human-readable format"
        FormatTime,

        #[text = ["current_date", "cur_date", "curdate"]]
        @weight = 1
        @group = "Datetime"
        @description = "Get the current date"
        CurrentDate,

        #[text = ["current_time", "cur_time", "curtime"]]
        @weight = 1
        @group = "Datetime"
        @description = "Get the current time (HH:MM:SS)"
        CurrentTime,

        #[text = ["current_timestamp", "now"]]
        @weight = 1
        @group = "Datetime"
        @description = "Get the current timestamp (YYYY-MM-DD HH:MM:SS)"
        CurrentTimestamp,

        #[text = ["day"], data_type = "numeric"]
        @group = "Datetime"
        @description = "Get the day from a date"
        Day,

        #[text = ["month"], data_type = "numeric"]
        @group = "Datetime"
        @description = "Get the month from a date"
        Month,

        #[text = ["year"], data_type = "numeric"]
        @group = "Datetime"
        @description = "Get the year from a date"
        Year,

        #[text = ["dayofweek", "dow"], data_type = "numeric"]
        @group = "Datetime"
        @description = "Get the day of the week from a date"
        DayOfWeek,

        #[text = ["current_uid"], data_type = "numeric"]
        @weight = 1
        @group = "Other"
        @description = "Get the current user ID"
        #[cfg(all(unix, feature = "users"))]
        CurrentUid,

        #[text = ["current_user"]]
        @weight = 1
        @group = "Other"
        @description = "Get the current username"
        #[cfg(all(unix, feature = "users"))]
        CurrentUser,

        #[text = ["current_gid"], data_type = "numeric"]
        @weight = 1
        @group = "Other"
        @description = "Get the current group ID"
        #[cfg(all(unix, feature = "users"))]
        CurrentGid,

        #[text = ["current_group"]]
        @weight = 1
        @group = "Other"
        @description = "Get the current group name"
        #[cfg(all(unix, feature = "users"))]
        CurrentGroup,

        #[text = ["contains"], data_type = "boolean"]
        @weight = 1024
        @group = "Other"
        @description = "Checks if a file contains a substring"
        Contains,

        #[text = ["has_xattr"], data_type = "boolean"]
        @weight = 2
        @group = "Xattr"
        @description = "Check if the file has a specific extended attribute"
        #[cfg(unix)]
        HasXattr,

        #[text = ["xattr"]]
        @weight = 2
        @group = "Xattr"
        @description = "Get the value of an extended attribute"
        #[cfg(unix)]
        Xattr,

        #[text = ["has_extattr"], data_type = "boolean"]
        @weight = 2
        @group = "Xattr"
        @description = "Check if the file has a specific extended file attribute flag"
        #[cfg(target_os = "linux")]
        HasExtattr,

        #[text = ["acl"]]
        @weight = 2
        @group = "Xattr"
        @description = "Get all POSIX ACL entries in standard form"
        #[cfg(target_os = "linux")]
        Acl,

        #[text = ["has_acl_entry"], data_type = "boolean"]
        @weight = 2
        @group = "Xattr"
        @description = "Check if a specific POSIX ACL entry exists"
        #[cfg(target_os = "linux")]
        HasAclEntry,

        #[text = ["acl_entry"]]
        @weight = 2
        @group = "Xattr"
        @description = "Get permissions of a specific POSIX ACL entry"
        #[cfg(target_os = "linux")]
        AclEntry,

        #[text = ["default_acl"]]
        @weight = 2
        @group = "Xattr"
        @description = "Get all default POSIX ACL entries in standard form"
        #[cfg(target_os = "linux")]
        DefaultAcl,

        #[text = ["has_default_acl_entry"], data_type = "boolean"]
        @weight = 2
        @group = "Xattr"
        @description = "Check if a specific default POSIX ACL entry exists"
        #[cfg(target_os = "linux")]
        HasDefaultAclEntry,

        #[text = ["default_acl_entry"]]
        @weight = 2
        @group = "Xattr"
        @description = "Get permissions of a specific default POSIX ACL entry"
        #[cfg(target_os = "linux")]
        DefaultAclEntry,

        #[text = ["has_capabilities", "has_caps"], data_type = "boolean"]
        @weight = 2
        @group = "Xattr"
        @description = "Check if the file has capabilities (security.capability xattr)"
        #[cfg(target_os = "linux")]
        HasCapabilities,

        #[text = ["has_capability", "has_cap"], data_type = "boolean"]
        @weight = 2
        @group = "Xattr"
        @description = "Check if the file has a specific capability (security.capability xattr)"
        #[cfg(target_os = "linux")]
        HasCapability,

        #[text = ["coalesce"]]
        @group = "Other"
        @description = "Return the first non-empty value"
        Coalesce,

        #[text = ["rand", "random"], data_type = "numeric"]
        @weight = 1
        @group = "Numeric"
        @description = "Gets a random number from 0 to the value, or between two values"
        Random,

        #[text = ["min"], data_type = "numeric"]
        @is_aggregate = true
        @group = "Aggregate"
        @description = "Get the minimum value"
        Min,

        #[text = ["max"], data_type = "numeric"]
        @is_aggregate = true
        @group = "Aggregate"
        @description = "Get the maximum value"
        Max,

        #[text = ["avg"], data_type = "numeric"]
        @is_aggregate = true
        @group = "Aggregate"
        @description = "Get the average value"
        Avg,

        #[text = ["sum"], data_type = "numeric"]
        @is_aggregate = true
        @group = "Aggregate"
        @description = "Get the sum of all values"
        Sum,

        #[text = ["count"], data_type = "numeric"]
        @is_aggregate = true
        @group = "Aggregate"
        @description = "Get the number of values"
        Count,

        #[text = ["stddev_pop", "stddev", "std"], data_type = "numeric"]
        @is_aggregate = true
        @group = "Aggregate"
        @description = "Get the population standard deviation"
        StdDevPop,

        #[text = ["stddev_samp"], data_type = "numeric"]
        @is_aggregate = true
        @group = "Aggregate"
        @description = "Get the sample standard deviation"
        StdDevSamp,

        #[text = ["var_pop", "variance"], data_type = "numeric"]
        @is_aggregate = true
        @group = "Aggregate"
        @description = "Get the population variance"
        VarPop,

        #[text = ["var_samp"], data_type = "numeric"]
        @is_aggregate = true
        @group = "Aggregate"
        @description = "Get the sample variance"
        VarSamp,
    }
}

#[derive(Debug, Default)]
pub struct FieldAccumulator {
    pub count: usize,
    pub min: f64,
    pub max: f64,
    pub sum: f64,
    pub m2: f64,
}

impl FieldAccumulator {
    pub fn push(&mut self, value: &str) {
        if let Ok(v) = value.parse::<f64>() {
            if v.is_finite() {
                if self.count == 0 {
                    self.min = v;
                    self.max = v;
                    self.count = 1;
                    self.sum = v;
                } else {
                    if v < self.min { self.min = v; }
                    if v > self.max { self.max = v; }
                    let old_mean = self.sum / self.count as f64;
                    self.count += 1;
                    self.sum += v;
                    let new_mean = self.sum / self.count as f64;
                    self.m2 += (v - old_mean) * (v - new_mean);
                }
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct GroupAccumulator {
    pub total_count: usize,
    pub fields: HashMap<String, FieldAccumulator>,
}

impl GroupAccumulator {
    pub fn increment_count(&mut self) {
        self.total_count += 1;
    }

    pub fn push(&mut self, field: &str, value: &str) {
        self.fields.entry(field.to_string()).or_default().push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn function_lower() {
        let function = Function::Lower;
        let function_arg = String::from("HELLO");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "hello");
    }
    
    #[test]
    fn function_upper() {
        let function = Function::Upper;
        let function_arg = String::from("hello");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "HELLO");
    }
    
    #[test]
    fn function_initcap() {
        let function = Function::InitCap;
        let function_arg = String::from("hello world");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "Hello World");
    }
    
    #[test]
    fn function_length() {
        let function = Function::Length;
        let function_arg = String::from("hello");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_int(), 5);
    }
    
    #[test]
    fn function_to_base64() {
        let function = Function::ToBase64;
        let function_arg = String::from("hello");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "aGVsbG8=");
    }
    
    #[test]
    fn function_from_base64() {
        let function = Function::FromBase64;
        let function_arg = String::from("aGVsbG8=");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "hello");
    }
    
    #[test]
    fn function_concat() {
        let function = Function::Concat;
        let function_arg = String::from("hello");
        let function_args = vec![String::from(" world")];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "hello world");
    }
    
    #[test]
    fn function_concat_ws() {
        let function = Function::ConcatWs;
        let function_arg = String::from(", ");
        let function_args = vec![String::from("hello"), String::from("world")];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "hello, world");
    }
    
    #[test]
    fn function_locate() {
        let function = Function::Locate;
        let function_arg = String::from("hello world");
        let function_args = vec![String::from("world"), String::from("1")];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_int(), 7);
    }
    
    #[test]
    fn function_substring() {
        let function = Function::Substring;
        let function_arg = String::from("hello world");
        let function_args = vec![String::from("7"), String::from("5")];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "world");
    }
    
    #[test]
    fn function_replace() {
        let function = Function::Replace;
        let function_arg = String::from("hello world");
        let function_args = vec![String::from("world"), String::from("Rust")];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "hello Rust");
    }
    
    #[test]
    fn function_trim() {
        let function = Function::Trim;
        let function_arg = String::from("   hello   ");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "hello");
    }
    
    #[test]
    fn function_ltrim() {
        let function = Function::LTrim;
        let function_arg = String::from("   hello   ");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "hello   ");
    }
    
    #[test]
    fn function_rtrim() {
        let function = Function::RTrim;
        let function_arg = String::from("   hello   ");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "   hello");
    }
    
    #[test]
    fn function_bin() {
        let function = Function::Bin;
        let function_arg = String::from("10");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "1010");
    }
    
    #[test]
    fn function_hex() {
        let function = Function::Hex;
        let function_arg = String::from("255");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "ff");
    }
    
    #[test]
    fn function_oct() {
        let function = Function::Oct;
        let function_arg = String::from("8");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "10");
    }
    
    #[test]
    fn function_abs() {
        let function = Function::Abs;
        let function_arg = String::from("-10");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_int(), 10);
    }
    
    #[test]
    fn function_power() {
        let function = Function::Power;
        let function_arg = String::from("2");
        let function_args = vec![String::from("3")];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_int(), 8);
    }
    
    #[test]
    fn function_sqrt() {
        let function = Function::Sqrt;
        let function_arg = String::from("16");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_int(), 4);
    }
    
    #[test]
    fn function_log() {
        let function = Function::Log;
        let function_arg = String::from("100");
        let function_args = vec![String::from("10")];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_int(), 2);
    }
    
    #[test]
    fn function_ln() {
        let function = Function::Ln;
        let function_arg = std::f64::consts::E.to_string();
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_int(), 1);
    }
    
    #[test]
    fn function_exp() {
        let function = Function::Exp;
        let function_arg = String::from("1");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_float(), std::f64::consts::E);
    }

    #[test]
    fn function_least() {
        let function = Function::Least;
        let function_arg = String::from("10");
        let function_args = vec![String::from("20"), String::from("30")];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_int(), 10);
    }
    
    #[test]
    fn function_greatest() {
        let function = Function::Greatest;
        let function_arg = String::from("10");
        let function_args = vec![String::from("20"), String::from("30")];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_int(), 30);
    }
    
    #[test]
    fn function_pi() {
        let function = Function::Pi;
        let function_arg = String::new();
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_float(), std::f64::consts::PI);
    }
    
    #[test]
    fn function_contains_japanese() {
        let function = Function::ContainsJapanese;
        let function_arg = String::from("こんにちは");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_bool(), true);
    }
    
    #[test]
    fn function_contains_hiragana() {
        let function = Function::ContainsHiragana;
        let function_arg = String::from("こんにちは");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_bool(), true);
    }
    
    #[test]
    fn function_contains_katakana() {
        let function = Function::ContainsKatakana;
        let function_arg = String::from("カタカナ");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_bool(), true);
    }
    
    #[test]
    fn function_contains_kana() {
        let function = Function::ContainsKana;
        let function_arg = String::from("カタカナ");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_bool(), true);
    }
    
    #[test]
    fn function_contains_kanji() {
        let function = Function::ContainsKanji;
        let function_arg = String::from("漢字");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_bool(), true);
    }
    
    #[test]
    fn function_contains_greek() {
        let function = Function::ContainsGreek;
        let function_arg = String::from("Ελληνικά");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_bool(), true);
    }
    
    #[test]
    fn function_format_size() {
        let function = Function::FormatSize;
        let function_arg = String::from("1024");
        let function_args = vec![String::from("%.0 k")];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "1 KiB");
    }
    
    #[test]
    fn function_format_time() {
        let function = Function::FormatTime;
        let function_arg = String::from("3600");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "1h");
    }
    
    #[test]
    fn function_current_date() {
        let function = Function::CurrentDate;
        let function_arg = String::new();
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), format_date(&Local::now().date_naive()));
    }
    
    #[test]
    fn function_current_timestamp() {
        let function = Function::CurrentTimestamp;
        let function_arg = String::new();
        let function_args: Vec<String> = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        let s = result.unwrap().to_string();
        // Expect format YYYY-MM-DD HH:MM:SS → length 19 and separators at fixed positions
        assert_eq!(s.len(), 19, "Unexpected CURRENT_TIMESTAMP length: {}", s);
        let bytes = s.as_bytes();
        assert_eq!(bytes[4] as char, '-', "expected '-' at pos 4 in {}", s);
        assert_eq!(bytes[7] as char, '-', "expected '-' at pos 7 in {}", s);
        assert_eq!(bytes[10] as char, ' ', "expected space at pos 10 in {}", s);
        assert_eq!(bytes[13] as char, ':', "expected ':' at pos 13 in {}", s);
        assert_eq!(bytes[16] as char, ':', "expected ':' at pos 16 in {}", s);
    }
    
    #[test]
    fn function_day() {
        let function = Function::Day;
        let function_arg = String::from("2023-10-01");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_int(), 1);
    }
    
    #[test]
    fn function_month() {
        let function = Function::Month;
        let function_arg = String::from("2023-10-01");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_int(), 10);
    }
    
    #[test]
    fn function_year() {
        let function = Function::Year;
        let function_arg = String::from("2023-10-01");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_int(), 2023);
    }
    
    #[test]
    fn function_day_of_week() {
        let function = Function::DayOfWeek;
        let function_arg = String::from("2023-10-01");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_int(), 1);
    }
    
    #[test]
    #[cfg(all(unix, feature = "users"))]
    fn function_current_uid() {
        let function = Function::CurrentUid;
        let function_arg = String::new();
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_int(), uzers::get_current_uid() as i64);
    }
    
    #[test]
    #[cfg(all(unix, feature = "users"))]
    fn function_current_user() {
        let function = Function::CurrentUser;
        let function_arg = String::new();
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), uzers::get_current_username().unwrap().to_string_lossy().to_string());
    }
    
    #[test]
    #[cfg(all(unix, feature = "users"))]
    fn function_current_gid() {
        let function = Function::CurrentGid;
        let function_arg = String::new();
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_int(), uzers::get_current_gid() as i64);
    }
    
    #[test]
    #[cfg(all(unix, feature = "users"))]
    fn function_current_group() {
        let function = Function::CurrentGroup;
        let function_arg = String::new();
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), uzers::get_current_groupname().unwrap().to_string_lossy().to_string());
    }
    
    #[test]
    fn function_coalesce() {
        let function = Function::Coalesce;
        let function_arg = String::new();
        let function_args = vec![String::new(), String::from("hello"), String::from("world")];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "hello");
    }

    fn make_accumulator(key: &str, values: &[&str]) -> GroupAccumulator {
        let mut acc = GroupAccumulator::default();
        for v in values {
            acc.increment_count();
            acc.push(key, v);
        }
        acc
    }

    #[test]
    fn avg_truncates_to_integer() {
        let acc = make_accumulator("size", &["3", "4"]);
        let result = get_aggregate_value(&Function::Avg, &acc, "size".to_string(), &None);
        assert_eq!(result, "3.5");
    }

    #[test]
    fn sum_ignores_negative_values() {
        let acc = make_accumulator("val", &["-5", "10"]);
        let result = get_aggregate_value(&Function::Sum, &acc, "val".to_string(), &None);
        assert_eq!(result, "5");
    }

    #[test]
    fn locate_wrong_position_for_multibyte() {
        let result = get_value(
            &Function::Locate,
            String::from("aéb"),
            vec![String::from("b")],
            None,
            &None,
        ).unwrap();
        assert_eq!(result.to_int(), 3);
    }

    #[test]
    fn initcap_collapses_multiple_spaces() {
        let result = get_value(
            &Function::InitCap,
            String::from("hello  world"),
            vec![],
            None,
            &None,
        ).unwrap();
        assert_eq!(result.to_string(), "Hello  World");
    }

    #[test]
    fn initcap_destroys_tab_separator() {
        let result = get_value(
            &Function::InitCap,
            String::from("hello\tworld"),
            vec![],
            None,
            &None,
        ).unwrap();
        assert_eq!(result.to_string(), "Hello\tWorld");
    }

    #[test]
    fn min_drops_fractional_values() {
        let acc = make_accumulator("val", &["1.5", "2.5"]);
        let result = get_aggregate_value(&Function::Min, &acc, "val".to_string(), &None);
        assert_eq!(result, "1.5");
    }

    #[test]
    fn max_drops_fractional_values() {
        let acc = make_accumulator("val", &["1.5", "2.5"]);
        let result = get_aggregate_value(&Function::Max, &acc, "val".to_string(), &None);
        assert_eq!(result, "2.5");
    }

    #[test]
    fn sum_drops_fractional_values() {
        let acc = make_accumulator("val", &["1.5", "2.5"]);
        let result = get_aggregate_value(&Function::Sum, &acc, "val".to_string(), &None);
        assert_eq!(result, "4");
    }

    #[test]
    fn avg_wrong_with_unparseable_entries() {
        let mut acc = make_accumulator("val", &["10", "20"]);
        acc.increment_count();
        acc.push("other_key", "999");
        let result = get_aggregate_value(&Function::Avg, &acc, "val".to_string(), &None);
        assert_eq!(result, "15");
    }

    #[test]
    fn variance_wrong_with_unparseable_entries() {
        let mut acc = make_accumulator("val", &["10", "20"]);
        acc.increment_count();
        acc.push("other_key", "999");
        let result = get_aggregate_value(&Function::VarPop, &acc, "val".to_string(), &None);
        let var: f64 = result.parse().unwrap();
        assert!((var - 25.0).abs() < 0.001);
    }

    #[test]
    fn format_time_panics_on_non_numeric() {
        let result = get_value(
            &Function::FormatTime,
            String::from("abc"),
            vec![],
            None,
            &None,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn random_panics_on_zero_upper_bound() {
        let result = get_value(
            &Function::Random,
            String::from("0"),
            vec![],
            None,
            &None,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn random_panics_on_inverted_range() {
        let result = get_value(
            &Function::Random,
            String::from("5"),
            vec![String::from("3")],
            None,
            &None,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn power_errors_on_non_numeric_exponent() {
        let result = get_value(
            &Function::Power,
            String::from("2"),
            vec![String::from("abc")],
            None,
            &None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn log_errors_on_non_numeric_base() {
        let result = get_value(
            &Function::Log,
            String::from("100"),
            vec![String::from("abc")],
            None,
            &None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn locate_errors_on_non_numeric_position() {
        let result = get_value(
            &Function::Locate,
            String::from("hello"),
            vec![String::from("l"), String::from("abc")],
            None,
            &None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn substring_errors_on_non_numeric_position() {
        let result = get_value(
            &Function::Substring,
            String::from("hello"),
            vec![String::from("abc")],
            None,
            &None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn substring_errors_on_non_numeric_length() {
        let result = get_value(
            &Function::Substring,
            String::from("hello"),
            vec![String::from("1"), String::from("abc")],
            None,
            &None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn sqrt_negative_returns_error() {
        let result = get_value(
            &Function::Sqrt,
            String::from("-1"),
            vec![],
            None,
            &None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn ln_zero_returns_error() {
        let result = get_value(
            &Function::Ln,
            String::from("0"),
            vec![],
            None,
            &None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn ln_negative_returns_error() {
        let result = get_value(
            &Function::Ln,
            String::from("-1"),
            vec![],
            None,
            &None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn log_base_one_returns_error() {
        let result = get_value(
            &Function::Log,
            String::from("100"),
            vec![String::from("1")],
            None,
            &None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn log_negative_value_returns_error() {
        let result = get_value(
            &Function::Log,
            String::from("-1"),
            vec![String::from("10")],
            None,
            &None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn power_negative_base_fractional_exp_returns_error() {
        let result = get_value(
            &Function::Power,
            String::from("-2"),
            vec![String::from("0.5")],
            None,
            &None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn power_zero_base_negative_exp_returns_error() {
        let result = get_value(
            &Function::Power,
            String::from("0"),
            vec![String::from("-1")],
            None,
            &None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn locate_no_search_arg_returns_error() {
        let result = get_value(
            &Function::Locate,
            String::from("hello"),
            vec![],
            None,
            &None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn replace_missing_args_returns_error() {
        let result = get_value(
            &Function::Replace,
            String::from("hello"),
            vec![],
            None,
            &None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn replace_missing_to_arg_returns_error() {
        let result = get_value(
            &Function::Replace,
            String::from("hello"),
            vec![String::from("l")],
            None,
            &None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn exp_overflow_returns_error() {
        let result = get_value(
            &Function::Exp,
            String::from("710"),
            vec![],
            None,
            &None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn substring_very_negative_pos_clamps() {
        let result = get_value(
            &Function::Substring,
            String::from("hello"),
            vec![String::from("-100"), String::from("3")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "hel");
    }

    #[test]
    fn locate_position_zero_finds_match() {
        let result = get_value(
            &Function::Locate,
            String::from("hello"),
            vec![String::from("h"), String::from("0")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "1");
    }

    #[test]
    fn substring_position_zero_with_length() {
        let result = get_value(
            &Function::Substring,
            String::from("hello"),
            vec![String::from("0"), String::from("3")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "hel");
    }

    #[test]
    fn substring_position_zero_without_length() {
        let result = get_value(
            &Function::Substring,
            String::from("hello"),
            vec![String::from("0")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "hello");
    }

    #[test]
    fn var_samp_single_value_is_empty() {
        let acc = make_accumulator("val", &["5"]);
        let result = get_aggregate_value(&Function::VarSamp, &acc, String::from("val"), &None);
        assert_eq!(result, String::new());
    }

    #[test]
    fn stddev_samp_single_value_is_empty() {
        let acc = make_accumulator("val", &["5"]);
        let result = get_aggregate_value(&Function::StdDevSamp, &acc, String::from("val"), &None);
        assert_eq!(result, String::new());
    }

    #[test]
    fn var_pop_no_parseable_values_is_empty() {
        let acc = make_accumulator("val", &["abc", "def"]);
        let result = get_aggregate_value(&Function::VarPop, &acc, String::from("val"), &None);
        assert_eq!(result, String::new());
    }

    #[test]
    fn min_no_parseable_values_is_empty() {
        let acc = make_accumulator("val", &["abc", "def"]);
        let result = get_aggregate_value(&Function::Min, &acc, String::from("val"), &None);
        assert_eq!(result, String::new());
    }

    #[test]
    fn max_no_parseable_values_is_empty() {
        let acc = make_accumulator("val", &["abc", "def"]);
        let result = get_aggregate_value(&Function::Max, &acc, String::from("val"), &None);
        assert_eq!(result, String::new());
    }

    #[test]
    fn avg_no_parseable_values_is_empty() {
        let acc = make_accumulator("val", &["abc", "def"]);
        let result = get_aggregate_value(&Function::Avg, &acc, String::from("val"), &None);
        assert_eq!(result, String::new());
    }

    #[test]
    fn initcap_hyphenated_words() {
        let result = get_value(
            &Function::InitCap,
            String::from("hello-world"),
            vec![],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "Hello-World");
    }

    #[test]
    fn initcap_underscore_words() {
        let result = get_value(
            &Function::InitCap,
            String::from("foo_bar_baz"),
            vec![],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "Foo_Bar_Baz");
    }

    #[test]
    fn substring_explicit_length_zero() {
        let result = get_value(
            &Function::Substring,
            String::from("hello"),
            vec![String::from("2"), String::from("0")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "");
    }

    #[test]
    fn replace_empty_search_returns_original() {
        let result = get_value(
            &Function::Replace,
            String::from("hello"),
            vec![String::from(""), String::from("x")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "hello");
    }

    #[test]
    fn ceil_negative_fractional_not_negative_zero() {
        let result = get_value(
            &Function::Ceil,
            String::from("-0.1"),
            vec![],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "0");
    }

    #[test]
    fn round_negative_fractional_not_negative_zero() {
        let result = get_value(
            &Function::Round,
            String::from("-0.4"),
            vec![],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "0");
    }

    #[test]
    fn round_with_precision_two() {
        let result = get_value(
            &Function::Round,
            String::from("3.14159"),
            vec![String::from("2")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_float(), 3.14);
    }

    #[test]
    fn round_with_precision_zero() {
        let result = get_value(
            &Function::Round,
            String::from("3.7"),
            vec![String::from("0")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_float(), 4.0);
    }

    #[test]
    fn round_with_negative_precision() {
        let result = get_value(
            &Function::Round,
            String::from("1234"),
            vec![String::from("-2")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_float(), 1200.0);
    }

    #[test]
    fn round_with_precision_no_arg_defaults_to_integer() {
        let result = get_value(
            &Function::Round,
            String::from("3.14159"),
            vec![],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_float(), 3.0);
    }

    #[test]
    fn round_with_invalid_precision_returns_error() {
        let result = get_value(
            &Function::Round,
            String::from("3.14"),
            vec![String::from("abc")],
            None,
            &None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn round_very_large_positive_precision() {
        let result = get_value(
            &Function::Round,
            String::from("3.14"),
            vec![String::from("309")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_float(), 3.14);
    }

    #[test]
    fn round_very_large_negative_precision() {
        let result = get_value(
            &Function::Round,
            String::from("1234"),
            vec![String::from("-400")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "0");
    }

    #[test]
    fn round_large_value_moderate_precision() {
        let result = get_value(
            &Function::Round,
            String::from("1e300"),
            vec![String::from("20")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_float(), 1e300);
    }

    #[test]
    fn floor_negative_zero_not_negative_zero() {
        let result = get_value(
            &Function::Floor,
            String::from("-0.0"),
            vec![],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "0");
    }

    #[test]
    fn sqrt_negative_zero_not_negative_zero() {
        let result = get_value(
            &Function::Sqrt,
            String::from("-0.0"),
            vec![],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "0");
    }

    #[test]
    fn abs_nan_string_returns_empty() {
        let result = get_value(
            &Function::Abs,
            String::from("NaN"),
            vec![],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "");
    }

    #[test]
    fn floor_nan_string_returns_empty() {
        let result = get_value(
            &Function::Floor,
            String::from("NaN"),
            vec![],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "");
    }

    #[test]
    fn ceil_inf_string_returns_empty() {
        let result = get_value(
            &Function::Ceil,
            String::from("inf"),
            vec![],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "");
    }

    #[test]
    fn random_upper_bound_is_inclusive() {
        let mut saw_max = false;
        for _ in 0..1000 {
            let result = get_value(
                &Function::Random,
                String::from("1"),
                vec![],
                None,
                &None,
            );
            let val = result.unwrap().to_int();
            assert!(val >= 0 && val <= 1);
            if val == 1 {
                saw_max = true;
            }
        }
        assert!(saw_max, "random(1) should be able to produce 1");
    }

    #[test]
    fn random_range_upper_bound_is_inclusive() {
        let mut saw_max = false;
        for _ in 0..1000 {
            let result = get_value(
                &Function::Random,
                String::from("5"),
                vec![String::from("6")],
                None,
                &None,
            );
            let val = result.unwrap().to_int();
            assert!(val >= 5 && val <= 6);
            if val == 6 {
                saw_max = true;
            }
        }
        assert!(saw_max, "random(5, 6) should be able to produce 6");
    }

    #[test]
    fn sum_no_parseable_values_is_empty() {
        let acc = make_accumulator("val", &["abc", "def"]);
        let result = get_aggregate_value(&Function::Sum, &acc, String::from("val"), &None);
        assert_eq!(result, String::new());
    }

    #[test]
    fn sum_ignores_nan_string_values() {
        let acc = make_accumulator("val", &["10", "NaN", "20"]);
        let result = get_aggregate_value(&Function::Sum, &acc, "val".to_string(), &None);
        assert_eq!(result, "30");
    }

    #[test]
    fn sum_ignores_inf_string_values() {
        let acc = make_accumulator("val", &["10", "inf", "20"]);
        let result = get_aggregate_value(&Function::Sum, &acc, "val".to_string(), &None);
        assert_eq!(result, "30");
    }

    #[test]
    fn avg_ignores_nan_string_values() {
        let acc = make_accumulator("val", &["10", "NaN", "20"]);
        let result = get_aggregate_value(&Function::Avg, &acc, "val".to_string(), &None);
        assert_eq!(result, "15");
    }

    #[test]
    fn var_pop_ignores_nan_string_values() {
        let acc = make_accumulator("val", &["10", "NaN", "20"]);
        let result = get_aggregate_value(&Function::VarPop, &acc, "val".to_string(), &None);
        let var: f64 = result.parse().unwrap();
        assert!((var - 25.0).abs() < 0.001);
    }

    #[test]
    fn sum_all_nan_is_empty() {
        let acc = make_accumulator("val", &["NaN", "NaN"]);
        let result = get_aggregate_value(&Function::Sum, &acc, "val".to_string(), &None);
        assert_eq!(result, String::new());
    }

    #[test]
    fn min_ignores_nan_string_values() {
        let acc = make_accumulator("val", &["10", "NaN", "5"]);
        let result = get_aggregate_value(&Function::Min, &acc, "val".to_string(), &None);
        assert_eq!(result, "5");
    }

    #[test]
    fn max_ignores_nan_string_values() {
        let acc = make_accumulator("val", &["10", "NaN", "5"]);
        let result = get_aggregate_value(&Function::Max, &acc, "val".to_string(), &None);
        assert_eq!(result, "10");
    }

    #[test]
    fn greatest_ignores_inf_in_args() {
        let result = get_value(
            &Function::Greatest,
            String::from("5"),
            vec![String::from("inf"), String::from("10")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), 10);
    }

    #[test]
    fn greatest_ignores_inf_as_function_arg() {
        let result = get_value(
            &Function::Greatest,
            String::from("inf"),
            vec![String::from("5"), String::from("10")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), 10);
    }

    #[test]
    fn least_ignores_neg_inf_in_args() {
        let result = get_value(
            &Function::Least,
            String::from("5"),
            vec![String::from("-inf"), String::from("3")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), 3);
    }

    #[test]
    fn least_ignores_neg_inf_as_function_arg() {
        let result = get_value(
            &Function::Least,
            String::from("-inf"),
            vec![String::from("5"), String::from("3")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), 3);
    }

    #[test]
    fn greatest_all_non_finite_returns_empty() {
        let result = get_value(
            &Function::Greatest,
            String::from("inf"),
            vec![String::from("NaN")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "");
    }

    #[test]
    fn least_ignores_nan_in_args() {
        let result = get_value(
            &Function::Least,
            String::from("5"),
            vec![String::from("NaN"), String::from("3")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), 3);
    }

    #[test]
    fn min_negative_zero_normalized() {
        let acc = make_accumulator("val", &["-0", "0"]);
        let result = get_aggregate_value(&Function::Min, &acc, "val".to_string(), &None);
        assert_eq!(result, "0");
    }

    #[test]
    fn max_negative_zero_normalized() {
        let acc = make_accumulator("val", &["-0", "0"]);
        let result = get_aggregate_value(&Function::Max, &acc, "val".to_string(), &None);
        assert_eq!(result, "0");
    }

    #[test]
    fn sum_overflow_returns_empty() {
        let acc = make_accumulator("val", &["1e308", "1e308"]);
        let result = get_aggregate_value(&Function::Sum, &acc, "val".to_string(), &None);
        assert_eq!(result, String::new());
    }

    #[test]
    fn avg_overflow_returns_empty() {
        let acc = make_accumulator("val", &["1e308", "1e308"]);
        let result = get_aggregate_value(&Function::Avg, &acc, "val".to_string(), &None);
        assert_eq!(result, String::new());
    }

    #[test]
    fn var_pop_overflow_returns_empty() {
        let acc = make_accumulator("val", &["1e308", "-1e308"]);
        let result = get_aggregate_value(&Function::VarPop, &acc, "val".to_string(), &None);
        assert_eq!(result, String::new());
    }

    #[test]
    fn stddev_pop_overflow_returns_empty() {
        let acc = make_accumulator("val", &["1e308", "-1e308"]);
        let result = get_aggregate_value(&Function::StdDevPop, &acc, "val".to_string(), &None);
        assert_eq!(result, String::new());
    }

    #[test]
    fn locate_empty_substring_beyond_string_length() {
        let result = get_value(
            &Function::Locate,
            String::from("hello"),
            vec![String::from(""), String::from("100")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), 0);
    }

    #[test]
    fn locate_beyond_string_length_returns_zero() {
        let result = get_value(
            &Function::Locate,
            String::from("hello"),
            vec![String::from("o"), String::from("100")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), 0);
    }

    #[test]
    fn locate_empty_substring_at_end_plus_one() {
        let result = get_value(
            &Function::Locate,
            String::from("hello"),
            vec![String::from(""), String::from("6")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), 6);
    }

    #[test]
    fn locate_empty_substring_past_end_plus_one() {
        let result = get_value(
            &Function::Locate,
            String::from("hello"),
            vec![String::from(""), String::from("7")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), 0);
    }

    #[test]
    fn locate_i32_min_position_no_overflow() {
        let result = get_value(
            &Function::Locate,
            String::from("hello"),
            vec![String::from("l"), String::from("-2147483648")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), 3);
    }
}