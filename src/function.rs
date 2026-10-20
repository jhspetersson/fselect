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

use chrono::Datelike;
use chrono::Local;
use chrono::LocalResult;
use chrono::DateTime;
use chrono::NaiveDate;
use chrono::NaiveDateTime;
use chrono::Timelike;
use rand::RngExt;
use serde::ser::{Serialize, Serializer};
#[cfg(unix)]
use xattr::FileExt;

use crate::fileinfo::{FileInfo, with_archived_entry};
use crate::util::{capitalize_initials, format_date, format_duration, format_time, format_datetime, parse_datetime, parse_filesize};
use std::cmp::Ordering;
use crate::util::variant::{Variant, VariantType};

macro_rules! functions {
    (
        #[group_order = [$($group_order:literal),*]$(,)?]
        $(#[$enum_attrs:meta])*
        $vis:vis enum $enum_name:ident {
            $(
                #[text = [$($text:literal),*]$(,)? $(data_type = $data_type:literal)?]
                $(@is_aggregate = $is_aggregate:literal)?
                $(@reads_entry = $reads_entry:literal)?
                $(@args = $args:literal)?
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
            
            pub fn is_datetime_function(&self) -> bool {
                match self {
                    $(
                        $(#[$variant_attrs])*
                        $enum_name::$variant => {
                            stringify!($($data_type)?) .replace("\"", "") == "datetime"
                        }
                    )*
                }
            }

            pub fn is_string_function(&self) -> bool {
                match self {
                    $(
                        $(#[$variant_attrs])*
                        $enum_name::$variant => {
                            stringify!($($data_type)?) .replace("\"", "") == "string"
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

            pub fn reads_entry(&self) -> bool {
                match self {
                    $(
                        $(#[$variant_attrs])*
                        $enum_name::$variant => {
                            stringify!($($reads_entry)?) == "true"
                        }
                    )*
                }
            }

            /// How many arguments the function accepts; exactly one unless
            /// the declaration says otherwise with `@args`.
            pub fn arity(&self) -> Arity {
                match self {
                    $(
                        $(#[$variant_attrs])*
                        $enum_name::$variant => Arity::parse(stringify!($($args)?)),
                    )*
                }
            }

            /// The first spelling of the function name, as used in messages.
            pub fn canonical_name(&self) -> &'static str {
                match self {
                    $(
                        $(#[$variant_attrs])*
                        $enum_name::$variant => [$($text),*][0],
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

/// Streams the file in fixed-size chunks searching for `needle`, with a
/// rolling overlap of `needle.len() - 1` bytes so matches that straddle chunk
/// boundaries are still found. Avoids the unbounded memory cost (and the
/// UTF-8 requirement) of `read_to_string`.
/// The needle in the file's own encoding, taken from its byte-order mark:
/// the encoded bytes, the length of the mark, and the code unit size the
/// match must be aligned to (a UTF-16LE `U+4241` is the bytes `41 42`, not
/// the text `AB`).
fn encoded_needle(head: &[u8], needle: &str) -> (Vec<u8>, usize, usize) {
    match crate::util::detect_bom(head) {
        Some(("UTF-16LE", len)) => (needle.encode_utf16().flat_map(|unit| unit.to_le_bytes()).collect(), len, 2),
        Some(("UTF-16BE", len)) => (needle.encode_utf16().flat_map(|unit| unit.to_be_bytes()).collect(), len, 2),
        Some(("UTF-32LE", len)) => (needle.chars().flat_map(|c| (c as u32).to_le_bytes()).collect(), len, 4),
        Some(("UTF-32BE", len)) => (needle.chars().flat_map(|c| (c as u32).to_be_bytes()).collect(), len, 4),
        Some((_, len)) => (needle.as_bytes().to_vec(), len, 1),
        None => (needle.as_bytes().to_vec(), 0, 1),
    }
}

fn latin1_needle(text: &str) -> Option<Vec<u8>> {
    text.chars().map(|c| u8::try_from(u32::from(c)).ok()).collect()
}

/// Without a byte-order mark the body is UTF-8 when it decodes as UTF-8 and
/// Latin-1 otherwise (a NUL makes it binary), exactly as `is_text`
/// classifies it. A non-ASCII needle is therefore looked for in both
/// spellings while the body is validated, and the spelling that fits the
/// whole body decides; an ASCII needle never gets here, its bytes are the
/// same in both.
fn contains_utf8_or_latin1<R: Read>(mut reader: R, head: &[u8], utf8: &[u8], latin1: Option<Vec<u8>>) -> bool {
    const CHUNK: usize = 8 * 1024;
    let longest = utf8.len().max(latin1.as_ref().map_or(0, Vec::len));
    let overlap = longest - 1;
    let mut buf = vec![0u8; (CHUNK + overlap).max(head.len())];
    buf[..head.len()].copy_from_slice(head);
    let mut carry = head.len();
    let mut fresh_from = 0;
    let utf8_finder = memchr::memmem::Finder::new(utf8);
    let latin1_finder = latin1.as_deref().map(memchr::memmem::Finder::new);
    let (mut utf8_match, mut latin1_match) = (false, false);
    let (mut valid_utf8, mut has_nul) = (true, false);
    let mut pending: Vec<u8> = Vec::new();

    loop {
        let end = match reader.read(&mut buf[carry..]) {
            Ok(0) if carry == 0 => break,
            Ok(n) => carry + n,
            Err(_) => return false,
        };
        let fresh = &buf[fresh_from..end];
        if !has_nul && fresh.contains(&0) {
            has_nul = true;
        }
        if valid_utf8 && !has_nul {
            pending.extend_from_slice(fresh);
            match std::str::from_utf8(&pending) {
                Ok(_) => pending.clear(),
                Err(err) if err.error_len().is_none() => {
                    pending.drain(..err.valid_up_to());
                }
                Err(_) => {
                    valid_utf8 = false;
                    pending.clear();
                }
            }
        }
        if !utf8_match && utf8_finder.find(&buf[..end]).is_some() {
            utf8_match = true;
        }
        if !latin1_match && latin1_finder.as_ref().is_some_and(|finder| finder.find(&buf[..end]).is_some()) {
            latin1_match = true;
        }
        // Only a match in both spellings is settled early: a Latin-1 match
        // still depends on no NUL turning up in a later chunk, which would
        // make the body binary and leave the raw byte search alone to count.
        if utf8_match && latin1_match {
            return true;
        }
        if end < buf.len() && end == carry {
            break;
        }
        if end >= overlap {
            buf.copy_within(end - overlap..end, 0);
            carry = overlap;
        } else {
            carry = end;
        }
        fresh_from = carry;
    }
    // An incomplete sequence left at the end of the file (a Latin-1 body
    // ending in a byte above 0x7F) is invalid UTF-8 as well.
    if !pending.is_empty() {
        valid_utf8 = false;
    }
    if valid_utf8 || has_nul {
        utf8_match
    } else {
        latin1_match
    }
}

fn file_contains<R: Read>(mut reader: R, needle_text: &str) -> bool {
    if needle_text.is_empty() {
        return true;
    }

    const CHUNK: usize = 8 * 1024;
    let mut head = [0u8; 4];
    let mut filled = 0;
    while filled < head.len() {
        match reader.read(&mut head[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(_) => return false,
        }
    }
    let (needle, bom_len, unit) = encoded_needle(&head[..filled], needle_text);
    if bom_len == 0 && !needle_text.is_ascii() {
        return contains_utf8_or_latin1(reader, &head[..filled], &needle, latin1_needle(needle_text));
    }

    let overlap = needle.len() - 1;
    let mut buf = vec![0u8; (CHUNK + overlap).max(filled)];
    buf[..filled].copy_from_slice(&head[..filled]);
    let mut carry = filled;
    let mut base = 0usize;
    // SIMD-accelerated substring search; the naive windows() scan was 3-4x
    // slower on cached files and O(n*m) on repetitive data.
    let finder = memchr::memmem::Finder::new(&needle);

    loop {
        let end = match reader.read(&mut buf[carry..]) {
            Ok(0) if carry == 0 => return false,
            Ok(n) => carry + n,
            Err(_) => return false,
        };
        // Matches may overlap: an unaligned occurrence can hide an aligned
        // one starting a byte later (UTF-16LE 上乎 is 0A 4E 4E 4E, and 乎 is
        // 4E 4E), so the scan resumes one byte after each rejected match.
        let mut from = 0;
        while let Some(pos) = finder.find(&buf[from..end]) {
            let at = from + pos;
            if (base + at).checked_sub(bom_len).is_some_and(|offset| offset % unit == 0) {
                return true;
            }
            from = at + 1;
        }
        if end < buf.len() && end == carry {
            return false;
        }
        if end >= overlap {
            buf.copy_within(end - overlap..end, 0);
            base += end - overlap;
            carry = overlap;
        } else {
            carry = end;
        }
    }
}

/// Parses `arg` as an `f64` and applies `f` to it. A value that does not parse
/// as a number yields an empty string variant, matching the behavior shared by
/// all the numeric functions.
/// LEAST / GREATEST. Empty (missing) arguments are skipped. When every
/// remaining argument is a number the comparison is numeric, with non-finite
/// values skipped too; otherwise all arguments are compared as strings, as in
/// MySQL. `pick` is the ordering a candidate must have against the current
/// best to replace it.
fn least_or_greatest(first: &str, rest: &[String], pick: Ordering) -> Variant {
    let values: Vec<&str> = std::iter::once(first)
        .chain(rest.iter().map(String::as_str))
        .filter(|value| !value.is_empty())
        .collect();

    let numbers: Option<Vec<f64>> = values.iter().map(|value| value.parse::<f64>().ok()).collect();
    match numbers {
        Some(numbers) => numbers
            .into_iter()
            .filter(|number| number.is_finite())
            .reduce(|best, number| if number.partial_cmp(&best) == Some(pick) { number } else { best })
            .map_or_else(|| Variant::empty(VariantType::String), Variant::from_float),
        None => values
            .into_iter()
            .reduce(|best, value| if value.cmp(best) == pick { value } else { best })
            .map_or_else(|| Variant::empty(VariantType::String), |value| Variant::from_string(&value.to_string())),
    }
}

/// 1-based character position of `needle` in `haystack`, searching from the
/// 1-based `start` when given; 0 when absent. Shared by LOCATE (needle first,
/// as in MySQL) and STRPOS (haystack first, as in PostgreSQL).
fn position_of(needle: &str, haystack: &str, start: Option<&String>, function_name: &str) -> Result<Variant, String> {
    let skip: usize = match start {
        Some(start) => match start.parse::<i32>() {
            Ok(start) => start.saturating_sub(1).max(0) as usize,
            Err(_) => return Err(format!("Could not parse position argument of {} function: {}", function_name, start)),
        },
        None => 0,
    };

    if skip > haystack.chars().count() {
        return Ok(Variant::from_int(0));
    }

    let searched: String = haystack.chars().skip(skip).collect();
    let position = searched
        .find(needle)
        .map(|byte_index| (searched[..byte_index].chars().count() + skip + 1) as i64)
        .unwrap_or(0);

    Ok(Variant::from_int(position))
}

/// The number of arguments a function accepts: `max` is `None` for
/// variadic functions.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Arity {
    pub min: usize,
    pub max: Option<usize>,
}

impl Arity {
    /// Reads an `@args` spec as the macro stringifies it: `"3"` for exactly
    /// three, `"1..3"` for a range, `"1.."` for at least one, and an empty
    /// spec for the default of exactly one.
    fn parse(spec: &str) -> Arity {
        let spec = spec.trim().trim_matches('"');
        if spec.is_empty() {
            return Arity { min: 1, max: Some(1) };
        }
        let bound = |s: &str| s.parse::<usize>().unwrap_or_else(|_| panic!("bad @args spec {:?}", spec));
        match spec.split_once("..") {
            Some((min, "")) => Arity { min: bound(min), max: None },
            Some((min, max)) => Arity { min: bound(min), max: Some(bound(max)) },
            None => Arity { min: bound(spec), max: Some(bound(spec)) },
        }
    }

    fn accepts(&self, given: usize) -> bool {
        given >= self.min && self.max.is_none_or(|max| given <= max)
    }
}

impl Display for Arity {
    fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
        let plural = |n: usize| if n == 1 { "argument" } else { "arguments" };
        match self.max {
            Some(max) if max == self.min && max == 0 => write!(f, "no arguments"),
            Some(max) if max == self.min => write!(f, "{} {}", max, plural(max)),
            Some(max) => write!(f, "{} to {} arguments", self.min, max),
            None => write!(f, "at least {} {}", self.min, plural(self.min)),
        }
    }
}

impl Function {
    pub fn passes_argument_type_through(&self) -> bool {
        matches!(self, Function::Min | Function::Max)
    }

    pub fn datetime_argument_positions(&self) -> &'static [usize] {
        match self {
            Function::Year
            | Function::Month
            | Function::Day
            | Function::DayOfWeek
            | Function::DayName
            | Function::DayOfYear
            | Function::LastDay
            | Function::DateAdd
            | Function::DateSub => &[0],
            Function::DateDiff => &[0, 1],
            Function::Extract | Function::DateTrunc => &[1],
            _ => &[],
        }
    }

    pub fn check_argument_count(&self, given: usize) -> Result<(), String> {
        let arity = self.arity();
        if arity.accepts(given) {
            Ok(())
        } else {
            Err(format!(
                "Function {} expects {}, got {}",
                self.canonical_name().to_uppercase(),
                arity,
                given
            ))
        }
    }
}

/// The extreme keeps the spelling it was pushed with (`000`, `-000`, `1e3`):
/// the result must name a value that exists, so no spelling is rewritten.
fn numeric_extreme_text(raw: &Option<String>) -> String {
    raw.clone().unwrap_or_default()
}

fn snap_to_exact_power(val: f64, base: f64, approx: f64) -> f64 {
    let rounded = approx.round();
    let exact = (approx - rounded).abs() < 1e-9
        && rounded.abs() <= i32::MAX as f64
        && base.powi(rounded as i32) == val;
    if exact { rounded } else { approx }
}

fn unary_float(arg: &str, f: impl FnOnce(f64) -> Result<Variant, String>) -> Result<Variant, String> {
    match arg.parse::<f64>() {
        Ok(val) => f(val),
        Err(_) => Ok(Variant::empty(VariantType::String)),
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
            let haystack = function_args
                .first()
                .ok_or_else(|| "LOCATE requires the string to search in as its second argument".to_string())?;
            position_of(&function_arg, haystack, function_args.get(1), "LOCATE")
        },
        Function::Strpos => {
            let needle = function_args
                .first()
                .ok_or_else(|| "STRPOS requires a search substring argument".to_string())?;
            position_of(needle, &function_arg, function_args.get(1), "STRPOS")
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

            let len: Option<i64> = match &function_args.get(1) {
                Some(len) => match len.parse::<i64>() {
                    Ok(l) => Some(l),
                    Err(_) => return Err(format!("Could not parse length argument of SUBSTRING function: {}", len)),
                },
                _ => None,
            };

            let result: String = match len {
                // MySQL returns an empty string for a non-positive length
                Some(l) if l <= 0 => String::new(),
                Some(l) => string.chars().skip(pos as usize).take(l as usize).collect(),
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
        Function::Abs => unary_float(&function_arg, |val| Ok(Variant::from_float(val.abs()))),
        Function::Power => unary_float(&function_arg, |val| {
            let power = match function_args.first() {
                Some(power) => match power.parse::<f64>() {
                    Ok(p) => p,
                    Err(_) => return Err(format!("Could not parse exponent argument of POWER function: {}", power)),
                },
                _ => 0.0,
            };

            let result = val.powf(power);
            if result.is_nan() || result.is_infinite() {
                return Ok(Variant::empty(VariantType::String));
            }
            Ok(Variant::from_float(result))
        }),
        Function::Sqrt => unary_float(&function_arg, |val| {
            if val < 0.0 {
                Ok(Variant::empty(VariantType::String))
            } else {
                Ok(Variant::from_float(val.sqrt()))
            }
        }),
        Function::Log => unary_float(&function_arg, |val| {
            let base = match function_args.first() {
                Some(base) => match base.parse::<f64>() {
                    Ok(b) => b,
                    Err(_) => return Err(format!("Could not parse base argument of LOG function: {}", base)),
                },
                _ => 10.0,
            };

            if val <= 0.0 {
                return Ok(Variant::empty(VariantType::String));
            }
            if !base.is_finite() || base <= 0.0 || base == 1.0 {
                return Ok(Variant::empty(VariantType::String));
            }

            let result = if base == 10.0 {
                val.log10()
            } else if base == 2.0 {
                val.log2()
            } else {
                snap_to_exact_power(val, base, val.ln() / base.ln())
            };
            Ok(Variant::from_float(result))
        }),
        Function::Ln => unary_float(&function_arg, |val| {
            if val <= 0.0 {
                Ok(Variant::empty(VariantType::String))
            } else {
                Ok(Variant::from_float(val.ln()))
            }
        }),
        Function::Exp => unary_float(&function_arg, |val| {
            let result = val.exp();
            if result.is_infinite() {
                return Ok(Variant::empty(VariantType::String));
            }
            Ok(Variant::from_float(result))
        }),
        Function::Least => Ok(least_or_greatest(&function_arg, &function_args, Ordering::Less)),
        Function::Greatest => Ok(least_or_greatest(&function_arg, &function_args, Ordering::Greater)),
        Function::Pi => {
            Ok(Variant::from_float(std::f64::consts::PI))
        }
        Function::Floor => unary_float(&function_arg, |val| Ok(Variant::from_float(val.floor()))),
        Function::Ceil => unary_float(&function_arg, |val| Ok(Variant::from_float(val.ceil()))),
        Function::Round => unary_float(&function_arg, |val| {
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
        }),

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
        Function::ContainsCyrillic => {
            Ok(Variant::from_bool(crate::util::cyrillic::contains_cyrillic(&function_arg)))
        }

        // ===== Formatting functions =====
        Function::FormatSize => {
            if function_arg.is_empty() {
                return Ok(Variant::empty(VariantType::String));
            }

            let size = function_arg
                .parse::<u64>()
                .ok()
                .or_else(|| {
                    function_arg
                        .parse::<f64>()
                        .ok()
                        .filter(|size| size.is_finite() && *size >= 0.0)
                        .map(|size| size.round() as u64)
                })
                .or_else(|| crate::util::parse_filesize(&function_arg));

            if let Some(size) = size {
                let modifier = match function_args.first() {
                    Some(modifier) => modifier,
                    _ => "",
                };
                let file_size = crate::util::format_filesize(size, modifier)?;
                return Ok(Variant::from_string(&file_size));
            }

            Ok(Variant::empty(VariantType::String))
        }
        Function::FormatTime => Ok(format_duration(&function_arg)
            .map_or_else(|| Variant::empty(VariantType::String), |formatted| Variant::from_string(&formatted))),

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
        Function::DayName => match parse_datetime(&function_arg) {
            Ok(date) => {
                let name = match date.0.weekday() {
                    chrono::Weekday::Mon => "Monday",
                    chrono::Weekday::Tue => "Tuesday",
                    chrono::Weekday::Wed => "Wednesday",
                    chrono::Weekday::Thu => "Thursday",
                    chrono::Weekday::Fri => "Friday",
                    chrono::Weekday::Sat => "Saturday",
                    chrono::Weekday::Sun => "Sunday",
                };
                Ok(Variant::from_string(&name.to_string()))
            }
            _ => Ok(Variant::empty(VariantType::String)),
        },
        Function::DayOfYear => match parse_datetime(&function_arg) {
            Ok(date) => Ok(Variant::from_int(date.0.ordinal() as i64)),
            _ => Ok(Variant::empty(VariantType::Int)),
        },
        Function::DateAdd => {
            if function_args.is_empty() {
                return Err("DATE_ADD requires a second argument (number of days)".to_string());
            }
            let days: i64 = match function_args[0].parse() {
                Ok(d) => d,
                Err(_) => return Err(format!("Invalid number of days: {}", function_args[0])),
            };
            match parse_datetime(&function_arg) {
                Ok(date) => {
                    match chrono::Duration::try_days(days).and_then(|duration| date.0.checked_add_signed(duration)) {
                        Some(result) => Ok(Variant::from_string(&format_datetime(&result))),
                        None => Err(format!("Number of days out of range: {}", days)),
                    }
                }
                _ => Ok(Variant::empty(VariantType::String)),
            }
        }
        Function::DateSub => {
            if function_args.is_empty() {
                return Err("DATE_SUB requires a second argument (number of days)".to_string());
            }
            let days: i64 = match function_args[0].parse() {
                Ok(d) => d,
                Err(_) => return Err(format!("Invalid number of days: {}", function_args[0])),
            };
            match parse_datetime(&function_arg) {
                Ok(date) => {
                    match chrono::Duration::try_days(days).and_then(|duration| date.0.checked_sub_signed(duration)) {
                        Some(result) => Ok(Variant::from_string(&format_datetime(&result))),
                        None => Err(format!("Number of days out of range: {}", days)),
                    }
                }
                _ => Ok(Variant::empty(VariantType::String)),
            }
        }
        Function::DateDiff => {
            if function_args.is_empty() {
                return Err("DATE_DIFF requires a second argument (date)".to_string());
            }
            let date1 = parse_datetime(&function_arg);
            let date2 = parse_datetime(&function_args[0]);
            match (date1, date2) {
                (Ok(d1), Ok(d2)) => {
                    // MySQL DATEDIFF ignores the time parts and diffs calendar dates
                    let diff = d1.0.date().signed_duration_since(d2.0.date()).num_days();
                    Ok(Variant::from_int(diff))
                }
                _ => Ok(Variant::empty(VariantType::Int)),
            }
        }
        Function::FromUnixtime => {
            let timestamp: i64 = match function_arg.parse() {
                Ok(t) => t,
                Err(_) => return Ok(Variant::empty(VariantType::String)),
            };
            match DateTime::from_timestamp(timestamp, 0) {
                // Render as local time: every other datetime in the tool is local-naive
                Some(dt) => Ok(Variant::from_string(&format_datetime(&dt.with_timezone(&Local).naive_local()))),
                None => Ok(Variant::empty(VariantType::String)),
            }
        }
        Function::LastDay => match parse_datetime(&function_arg) {
            Ok(date) => {
                let y = date.0.year();
                let m = date.0.month();
                // First day of next month minus one day
                let last = if m == 12 {
                    chrono::NaiveDate::from_ymd_opt(y + 1, 1, 1)
                } else {
                    chrono::NaiveDate::from_ymd_opt(y, m + 1, 1)
                };
                match last.and_then(|d| d.pred_opt()) {
                    Some(last_day) => {
                        Ok(Variant::from_string(&format_date(&last_day)))
                    }
                    None => Ok(Variant::empty(VariantType::String)),
                }
            }
            _ => Ok(Variant::empty(VariantType::String)),
        },
        Function::Extract => {
            if function_args.is_empty() {
                return Err("EXTRACT requires two arguments: EXTRACT(unit, date)".to_string());
            }
            let dt = match parse_datetime(&function_args[0]) {
                Ok(d) => d.0,
                _ => return Ok(Variant::empty(VariantType::Int)),
            };
            let unit = function_arg.to_ascii_lowercase();
            match unit.as_str() {
                "year" | "years" | "yr" => Ok(Variant::from_int(dt.year() as i64)),
                "quarter" | "quarters" => Ok(Variant::from_int(((dt.month() - 1) / 3 + 1) as i64)),
                "month" | "months" | "mon" => Ok(Variant::from_int(dt.month() as i64)),
                "week" | "weeks" => Ok(Variant::from_int(dt.iso_week().week() as i64)),
                "day" | "days" => Ok(Variant::from_int(dt.day() as i64)),
                "hour" | "hours" => Ok(Variant::from_int(dt.hour() as i64)),
                "minute" | "minutes" | "min" => Ok(Variant::from_int(dt.minute() as i64)),
                "second" | "seconds" | "sec" => Ok(Variant::from_int(dt.second() as i64)),
                "dow" | "dayofweek" => Ok(Variant::from_int(dt.weekday().number_from_sunday() as i64)),
                "isodow" => Ok(Variant::from_int(dt.weekday().number_from_monday() as i64)),
                "doy" | "dayofyear" => Ok(Variant::from_int(dt.ordinal() as i64)),
                "epoch" | "unixtime" => {
                    // Naive datetimes in the tool are local time, so interpret them
                    // as local when computing the Unix timestamp
                    let timestamp = match dt.and_local_timezone(Local) {
                        LocalResult::Single(local) => local.timestamp(),
                        LocalResult::Ambiguous(earliest, _) => earliest.timestamp(),
                        // DST gap: no local mapping exists, fall back to UTC
                        LocalResult::None => dt.and_utc().timestamp(),
                    };
                    Ok(Variant::from_int(timestamp))
                }
                _ => Err(format!("Unsupported EXTRACT unit: {}", function_arg)),
            }
        }
        Function::DateTrunc => {
            if function_args.is_empty() {
                return Err("DATE_TRUNC requires two arguments: DATE_TRUNC(unit, date)".to_string());
            }
            let dt = match parse_datetime(&function_args[0]) {
                Ok(d) => d.0,
                _ => return Ok(Variant::empty(VariantType::String)),
            };
            let unit = function_arg.to_ascii_lowercase();
            let truncated: Option<NaiveDateTime> = match unit.as_str() {
                "year" | "years" => NaiveDate::from_ymd_opt(dt.year(), 1, 1)
                    .and_then(|d| d.and_hms_opt(0, 0, 0)),
                "quarter" | "quarters" => {
                    let m = ((dt.month() - 1) / 3) * 3 + 1;
                    NaiveDate::from_ymd_opt(dt.year(), m, 1).and_then(|d| d.and_hms_opt(0, 0, 0))
                }
                "month" | "months" => NaiveDate::from_ymd_opt(dt.year(), dt.month(), 1)
                    .and_then(|d| d.and_hms_opt(0, 0, 0)),
                "week" | "weeks" => {
                    // Truncate to Monday (ISO week start)
                    let offset = dt.weekday().num_days_from_monday() as i64;
                    let date = dt.date();
                    chrono::Duration::try_days(offset)
                        .and_then(|dur| date.checked_sub_signed(dur))
                        .and_then(|d| d.and_hms_opt(0, 0, 0))
                }
                "day" | "days" => dt.date().and_hms_opt(0, 0, 0),
                "hour" | "hours" => dt.date().and_hms_opt(dt.hour(), 0, 0),
                "minute" | "minutes" => dt.date().and_hms_opt(dt.hour(), dt.minute(), 0),
                "second" | "seconds" => Some(dt),
                _ => return Err(format!("Unsupported DATE_TRUNC unit: {}", function_arg)),
            };
            match truncated {
                Some(t) => Ok(Variant::from_string(&format_datetime(&t))),
                None => Ok(Variant::empty(VariantType::String)),
            }
        }

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
            let needle = function_arg.as_str();
            let found = match (entry, file_info) {
                (Some(entry), Some(file_info)) => {
                    with_archived_entry(&entry.path(), file_info, |reader| file_contains(reader, needle))
                }
                (Some(entry), None) => File::open(entry.path()).ok().map(|file| file_contains(file, needle)),
                (None, _) => None,
            };
            Ok(found.map_or_else(|| Variant::empty(VariantType::Bool), Variant::from_bool))
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
        #[cfg(windows)]
        Function::HasXattr => {
            if let Some(entry) = entry {
                return Ok(Variant::from_bool(
                    crate::util::win_xattr::has_named_ads(&entry.path(), &function_arg),
                ));
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
        #[cfg(windows)]
        Function::Xattr => {
            if let Some(entry) = entry
                && let Some(value) =
                    crate::util::win_xattr::read_named_ads(&entry.path(), &function_arg)
                {
                    return Ok(Variant::from_string(&value));
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
        #[cfg(windows)]
        Function::HasExtattr => {
            if let Some(entry) = entry
                && let Ok(meta) = entry.metadata() {
                    let attrs = crate::util::win_attrs::get_attrs(&meta);
                    return Ok(Variant::from_bool(
                        crate::util::win_attrs::has_attr(attrs, &function_arg),
                    ));
                }

            Ok(Variant::empty(VariantType::Bool))
        }
        #[cfg(target_os = "linux")]
        Function::HasAclEntry => {
            if let Some(entry) = entry {
                if let Ok(file) = File::open(entry.path()) {
                    if let Ok(Some(acl_data)) = file.get_xattr("system.posix_acl_access") {
                        if let Some(entries) = crate::util::acl::parse_acl(&acl_data) {
                            return Ok(Variant::from_bool(
                                crate::util::acl::with_shared_names(|names| crate::util::acl::find_entry(&entries, &function_arg, names).is_some()),
                            ));
                        }
                    }
                }
            }

            Ok(Variant::empty(VariantType::Bool))
        }
        #[cfg(windows)]
        Function::HasAclEntry => {
            if let Some(entry) = entry {
                return Ok(Variant::from_bool(
                    crate::util::win_acl::find_acl_entry(&entry.path(), &function_arg).is_some(),
                ));
            }

            Ok(Variant::empty(VariantType::Bool))
        }
        #[cfg(target_os = "linux")]
        Function::AclEntry => {
            if let Some(entry) = entry {
                if let Ok(file) = File::open(entry.path()) {
                    if let Ok(Some(acl_data)) = file.get_xattr("system.posix_acl_access") {
                        if let Some(entries) = crate::util::acl::parse_acl(&acl_data) {
                            let formatted = crate::util::acl::with_shared_names(|names| {
                                crate::util::acl::find_entry(&entries, &function_arg, names)
                                    .map(|acl_entry| crate::util::acl::format_entry(acl_entry, names))
                            });
                            if let Some(formatted) = formatted {
                                return Ok(Variant::from_string(&formatted));
                            }
                        }
                    }
                }
            }

            Ok(Variant::empty(VariantType::String))
        }
        #[cfg(windows)]
        Function::AclEntry => {
            if let Some(entry) = entry
                && let Some(acl_entry) =
                    crate::util::win_acl::find_acl_entry(&entry.path(), &function_arg)
                {
                    return Ok(Variant::from_string(&acl_entry));
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
                                    crate::util::acl::with_shared_names(|names| crate::util::acl::find_entry(&entries, &function_arg, names).is_some()),
                                ));
                            }
                        }
                    }
                }
            }

            Ok(Variant::empty(VariantType::Bool))
        }
        #[cfg(windows)]
        Function::HasDefaultAclEntry => {
            if let Some(entry) = entry
                && entry.path().is_dir() {
                    return Ok(Variant::from_bool(
                        crate::util::win_acl::find_default_acl_entry(&entry.path(), &function_arg)
                            .is_some(),
                    ));
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
                                let formatted = crate::util::acl::with_shared_names(|names| {
                                    crate::util::acl::find_entry(&entries, &function_arg, names)
                                        .map(|acl_entry| crate::util::acl::format_entry(acl_entry, names))
                                });
                                if let Some(formatted) = formatted {
                                    return Ok(Variant::from_string(&formatted));
                                }
                            }
                        }
                    }
                }
            }

            Ok(Variant::empty(VariantType::String))
        }
        #[cfg(windows)]
        Function::DefaultAclEntry => {
            if let Some(entry) = entry
                && entry.path().is_dir()
                && let Some(acl_entry) =
                    crate::util::win_acl::find_default_acl_entry(&entry.path(), &function_arg)
                {
                    return Ok(Variant::from_string(&acl_entry));
                }

            Ok(Variant::empty(VariantType::String))
        }
        #[cfg(target_os = "linux")]
        Function::HasCapability => {
            if let Some(entry) = entry {
                if let Ok(file) = File::open(entry.path()) {
                    if let Ok(Some(caps_xattr)) = file.get_xattr("security.capability") {
                        let caps_string = crate::util::capabilities::parse_capabilities(caps_xattr);
                        return Ok(Variant::from_bool(crate::util::capabilities::has_capability(&caps_string, &function_arg)));
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
                                let (lo, hi) = if val <= limit { (val, limit) } else { (limit, val) };
                                Ok(Variant::from_int(rng.random_range(lo..=hi)))
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
                // Numeric order when every value was a number, size order when
                // every value was a file size, plain text order otherwise
                // (datetimes, names, mixed columns).
                Some(acc) if acc.all_numeric() => numeric_extreme_text(&acc.min_raw),
                Some(acc) if acc.all_sizes() => acc.min_value.clone().unwrap_or_default(),
                Some(acc) => acc.min_text.clone().unwrap_or_default(),
                _ => String::new(),
            }
        }
        Function::Max => {
            match field_acc {
                Some(acc) if acc.all_numeric() => numeric_extreme_text(&acc.max_raw),
                Some(acc) if acc.all_sizes() => acc.max_value.clone().unwrap_or_default(),
                Some(acc) => acc.max_text.clone().unwrap_or_default(),
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
        Function::Count => {
            // COUNT(*) and bare COUNT count all rows in the group; COUNT(col)
            // follows SQL semantics and skips empty (NULL-like) values.
            if buffer_key == "*" || buffer_key.is_empty() {
                accumulator.total_count.to_string()
            } else {
                match field_acc {
                    Some(acc) => acc.non_empty_count.to_string(),
                    None => accumulator.total_count.to_string(),
                }
            }
        }
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
    #[group_order = ["String", "Japanese string", "Greek string", "Cyrillic string", "Numeric", "Datetime", "Aggregate", "Xattr", "Other"]]
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Hash)]
    pub enum Function {
        #[text = ["lower", "lowercase", "lcase"], data_type = "string"]
        @group = "String"
        @description = "Convert the value to lowercase"
        Lower,

        #[text = ["upper", "uppercase", "ucase"], data_type = "string"]
        @group = "String"
        @description = "Convert the value to UPPERCASE"
        Upper,

        #[text = ["initcap"], data_type = "string"]
        @group = "String"
        @description = "Capitalize the first letter of each word (Title Case)"
        InitCap,

        #[text = ["length", "len"], data_type = "numeric"]
        @group = "String"
        @description = "Get the length of the string"
        Length,

        #[text = ["to_base64", "base64"], data_type = "string"]
        @group = "String"
        @description = "Convert the value to base64"
        ToBase64,

        #[text = ["from_base64"], data_type = "string"]
        @group = "String"
        @description = "Read the value as base64"
        FromBase64,

        #[text = ["concat"], data_type = "string"]
        @args = "1.."
        @group = "String"
        @description = "Concatenate the value with the arguments"
        Concat,

        #[text = ["concat_ws"], data_type = "string"]
        @args = "2.."
        @group = "String"
        @description = "Concatenate the arguments, separated by the value"
        ConcatWs,

        #[text = ["locate", "position"], data_type = "numeric"]
        @args = "2..3"
        @group = "String"
        @description = "Get the position of the value (a substring) in the second argument, MySQL argument order"
        Locate,

        #[text = ["strpos"], data_type = "numeric"]
        @args = "2..3"
        @group = "String"
        @description = "Get the position of the second argument (a substring) in the value, PostgreSQL argument order"
        Strpos,

        #[text = ["substring", "substr"], data_type = "string"]
        @args = "2..3"
        @group = "String"
        @description = "Get a substring of the value, from a position and length"
        Substring,

        #[text = ["replace"], data_type = "string"]
        @args = "3"
        @group = "String"
        @description = "Replace a substring in the value with another string"
        Replace,

        #[text = ["trim"], data_type = "string"]
        @group = "String"
        @description = "Trim whitespace from the value"
        Trim,

        #[text = ["ltrim"], data_type = "string"]
        @group = "String"
        @description = "Trim whitespace from the start of the value"
        LTrim,

        #[text = ["rtrim"], data_type = "string"]
        @group = "String"
        @description = "Trim whitespace from the end of the value"
        RTrim,

        #[text = ["bin"], data_type = "string"]
        @group = "Numeric"
        @description = "Get the binary representation of the value"
        Bin,

        #[text = ["hex"], data_type = "string"]
        @group = "Numeric"
        @description = "Get the hexadecimal representation of the value"
        Hex,

        #[text = ["oct"], data_type = "string"]
        @group = "Numeric"
        @description = "Get the octal representation of the value"
        Oct,

        #[text = ["abs"], data_type = "numeric"]
        @group = "Numeric"
        @description = "Get the absolute value of the number"
        Abs,

        #[text = ["power", "pow"], data_type = "numeric"]
        @args = "2"
        @group = "Numeric"
        @description = "Raise the value to the power of another value"
        Power,

        #[text = ["sqrt"], data_type = "numeric"]
        @group = "Numeric"
        @description = "Get the square root of the value"
        Sqrt,

        #[text = ["log"], data_type = "numeric"]
        @args = "1..2"
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
        @args = "1.."
        @group = "Numeric"
        @description = "Get the smallest value"
        Least,

        #[text = ["greatest"], data_type = "numeric"]
        @args = "1.."
        @group = "Numeric"
        @description = "Get the largest value"
        Greatest,

        #[text = ["pi"], data_type = "numeric"]
        @args = "0"
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
        @args = "1..2"
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

        #[text = ["contains_cyrillic", "cyrillic"], data_type = "boolean"]
        @group = "Cyrillic string"
        @description = "Check if the string contains Cyrillic characters"
        ContainsCyrillic,

        #[text = ["format_size", "format_filesize"], data_type = "string"]
        @args = "1..2"
        @group = "Other"
        @description = "Format a file size in human-readable format"
        FormatSize,

        #[text = ["format_time", "pretty_time"], data_type = "string"]
        @group = "Other"
        @description = "Format a time duration in human-readable format"
        FormatTime,

        #[text = ["current_date", "cur_date", "curdate"], data_type = "datetime"]
        @args = "0"
        @group = "Datetime"
        @description = "Get the current date"
        CurrentDate,

        #[text = ["current_time", "cur_time", "curtime"], data_type = "string"]
        @args = "0"
        @group = "Datetime"
        @description = "Get the current time (HH:MM:SS)"
        CurrentTime,

        #[text = ["current_timestamp", "now"], data_type = "datetime"]
        @args = "0"
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

        #[text = ["dayname"], data_type = "string"]
        @group = "Datetime"
        @description = "Get the name of the day of the week from a date"
        DayName,

        #[text = ["dayofyear", "doy"], data_type = "numeric"]
        @group = "Datetime"
        @description = "Get the day of the year from a date (1-366)"
        DayOfYear,

        #[text = ["date_add", "dateadd"], data_type = "datetime"]
        @args = "2"
        @group = "Datetime"
        @description = "Add days to a date and return the resulting date"
        DateAdd,

        #[text = ["date_sub", "datesub"], data_type = "datetime"]
        @args = "2"
        @group = "Datetime"
        @description = "Subtract days from a date and return the resulting date"
        DateSub,

        #[text = ["date_diff", "datediff"], data_type = "numeric"]
        @args = "2"
        @group = "Datetime"
        @description = "Get the number of days between two dates"
        DateDiff,

        #[text = ["from_unixtime"], data_type = "datetime"]
        @group = "Datetime"
        @description = "Convert a Unix timestamp to a datetime string"
        FromUnixtime,

        #[text = ["last_day", "last_date"], data_type = "datetime"]
        @group = "Datetime"
        @description = "Get the last day of the month for a given date"
        LastDay,

        #[text = ["extract"], data_type = "numeric"]
        @args = "2"
        @group = "Datetime"
        @description = "Extract a date/time part (year, month, day, hour, minute, second, week, quarter, dow, doy, epoch) from a date. Usage: EXTRACT('year', date)"
        Extract,

        #[text = ["date_trunc", "datetrunc"], data_type = "datetime"]
        @args = "2"
        @group = "Datetime"
        @description = "Truncate a date to a unit (year, quarter, month, week, day, hour, minute, second). Usage: DATE_TRUNC('day', date)"
        DateTrunc,

        #[text = ["current_uid"], data_type = "numeric"]
        @args = "0"
        @group = "Other"
        @description = "Get the current user ID"
        #[cfg(all(unix, feature = "users"))]
        CurrentUid,

        #[text = ["current_user"], data_type = "string"]
        @args = "0"
        @group = "Other"
        @description = "Get the current username"
        #[cfg(all(unix, feature = "users"))]
        CurrentUser,

        #[text = ["current_gid"], data_type = "numeric"]
        @args = "0"
        @group = "Other"
        @description = "Get the current group ID"
        #[cfg(all(unix, feature = "users"))]
        CurrentGid,

        #[text = ["current_group"], data_type = "string"]
        @args = "0"
        @group = "Other"
        @description = "Get the current group name"
        #[cfg(all(unix, feature = "users"))]
        CurrentGroup,

        #[text = ["contains"], data_type = "boolean"]
        @reads_entry = true
        @weight = 1024
        @group = "Other"
        @description = "Checks if a file contains a substring"
        Contains,

        #[text = ["has_xattr"], data_type = "boolean"]
        @reads_entry = true
        @weight = 2
        @group = "Xattr"
        @description = "Check if the file has a specific extended attribute"
        #[cfg(any(unix, windows))]
        HasXattr,

        #[text = ["xattr"], data_type = "string"]
        @reads_entry = true
        @weight = 2
        @group = "Xattr"
        @description = "Get the value of an extended attribute"
        #[cfg(any(unix, windows))]
        Xattr,

        #[text = ["has_extattr"], data_type = "boolean"]
        @reads_entry = true
        @weight = 2
        @group = "Xattr"
        @description = "Check if the file has a specific extended file attribute flag"
        #[cfg(any(target_os = "linux", windows))]
        HasExtattr,

        #[text = ["has_acl_entry"], data_type = "boolean"]
        @reads_entry = true
        @weight = 2
        @group = "Xattr"
        @description = "Check if a specific POSIX ACL entry exists"
        #[cfg(any(target_os = "linux", windows))]
        HasAclEntry,

        #[text = ["acl_entry"], data_type = "string"]
        @reads_entry = true
        @weight = 2
        @group = "Xattr"
        @description = "Get permissions of a specific POSIX ACL entry"
        #[cfg(any(target_os = "linux", windows))]
        AclEntry,

        #[text = ["has_default_acl_entry"], data_type = "boolean"]
        @reads_entry = true
        @weight = 2
        @group = "Xattr"
        @description = "Check if a specific default POSIX ACL entry exists"
        #[cfg(any(target_os = "linux", windows))]
        HasDefaultAclEntry,

        #[text = ["default_acl_entry"], data_type = "string"]
        @reads_entry = true
        @weight = 2
        @group = "Xattr"
        @description = "Get permissions of a specific default POSIX ACL entry"
        #[cfg(any(target_os = "linux", windows))]
        DefaultAclEntry,

        #[text = ["has_capability", "has_cap"], data_type = "boolean"]
        @reads_entry = true
        @weight = 2
        @group = "Xattr"
        @description = "Check if the file has a specific capability (security.capability xattr)"
        #[cfg(target_os = "linux")]
        HasCapability,

        #[text = ["coalesce"]]
        @args = "1.."
        @group = "Other"
        @description = "Return the first non-empty value"
        Coalesce,

        #[text = ["rand", "random"], data_type = "numeric"]
        @args = "0..2"
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
        @args = "0..1"
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
    /// Number of pushed values that were not empty, regardless of whether they
    /// were numeric. Backs SQL-style `COUNT(col)`, which skips NULL-like values.
    pub non_empty_count: usize,
    pub min: f64,
    pub max: f64,
    pub sum: f64,
    pub m2: f64,
    /// Lexicographic extremes over the non-empty raw values. Back MIN/MAX for
    /// non-numeric columns: datetimes are formatted as fixed-width
    /// `YYYY-MM-DD HH:MM:SS`, so string order equals chronological order, and
    /// plain strings get SQL-style lexicographic MIN/MAX.
    pub min_value: Option<String>,
    pub max_value: Option<String>,
    min_size: Option<u64>,
    max_size: Option<u64>,
    numeric_like_count: usize,
    size_count: usize,
    min_text: Option<String>,
    max_text: Option<String>,
    min_raw: Option<String>,
    max_raw: Option<String>,
}

impl FieldAccumulator {
    pub fn push(&mut self, value: &str) {
        if !value.is_empty() {
            self.non_empty_count += 1;
            if value.parse::<f64>().is_ok() {
                self.numeric_like_count += 1;
            }
            if self.min_text.as_deref().is_none_or(|current| value < current) {
                self.min_text = Some(value.to_string());
            }
            if self.max_text.as_deref().is_none_or(|current| value > current) {
                self.max_text = Some(value.to_string());
            }
            let size = parse_filesize(value);
            if size.is_some() {
                self.size_count += 1;
            }
            if matches!(Self::compare(value, size, &self.min_value, self.min_size), None | Some(Ordering::Less)) {
                self.min_value = Some(value.to_string());
                self.min_size = size;
            }
            if matches!(Self::compare(value, size, &self.max_value, self.max_size), None | Some(Ordering::Greater)) {
                self.max_value = Some(value.to_string());
                self.max_size = size;
            }
        }
        if let Ok(v) = value.parse::<f64>()
            && v.is_finite() {
                if self.count == 0 {
                    self.min = v;
                    self.max = v;
                    self.min_raw = Some(value.to_string());
                    self.max_raw = Some(value.to_string());
                    self.count = 1;
                    self.sum = v;
                } else {
                    if v < self.min {
                        self.min = v;
                        self.min_raw = Some(value.to_string());
                    }
                    if v > self.max {
                        self.max = v;
                        self.max_raw = Some(value.to_string());
                    }
                    let old_mean = self.sum / self.count as f64;
                    self.count += 1;
                    self.sum += v;
                    let new_mean = self.sum / self.count as f64;
                    self.m2 += (v - old_mean) * (v - new_mean);
                }
            }
    }
}

impl FieldAccumulator {
    fn all_numeric(&self) -> bool {
        self.count > 0 && self.numeric_like_count == self.non_empty_count
    }

    fn all_sizes(&self) -> bool {
        self.size_count == self.non_empty_count
    }

    fn compare(value: &str, size: Option<u64>, current: &Option<String>, current_size: Option<u64>) -> Option<Ordering> {
        let current = current.as_ref()?;
        Some(match (size, current_size) {
            (Some(size), Some(current_size)) => size.cmp(&current_size),
            _ => value.cmp(current.as_str()),
        })
    }
}

#[derive(Debug, Default)]
pub struct GroupAccumulator {
    pub total_count: usize,
    pub fields: HashMap<String, FieldAccumulator>,
    /// Absolute path and root depth of the first entry that joined the group.
    /// Only recorded when the query drives an outer FROM-subselect, so each
    /// group can stand in for one filesystem entry there.
    pub representative: Option<(String, usize, Option<usize>)>,
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
// 3.14 appears as arbitrary float test input, not as an approximation of PI.
#[allow(clippy::approx_constant)]
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
        let function_arg = String::from("world");
        let function_args = vec![String::from("hello world"), String::from("1")];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_int(), 7);
    }
    
    #[test]
    fn arity_specs_cover_exact_open_and_bounded_ranges() {
        assert_eq!(Arity::parse(""), Arity { min: 1, max: Some(1) }, "unspecified means exactly one");
        assert_eq!(Arity::parse("\"0\""), Arity { min: 0, max: Some(0) });
        assert_eq!(Arity::parse("\"2..3\""), Arity { min: 2, max: Some(3) });
        assert_eq!(Arity::parse("\"1..\""), Arity { min: 1, max: None });
    }

    #[test]
    fn function_arities_match_their_implementations() {
        for (function, min, max) in [
            (Function::Lower, 1, Some(1)),
            (Function::Pi, 0, Some(0)),
            (Function::Coalesce, 1, None),
            (Function::ConcatWs, 2, None),
            (Function::Substring, 2, Some(3)),
            (Function::Locate, 2, Some(3)),
            (Function::Replace, 3, Some(3)),
            (Function::DateAdd, 2, Some(2)),
            (Function::Round, 1, Some(2)),
            (Function::Random, 0, Some(2)),
            (Function::Count, 0, Some(1)),
            (Function::Min, 1, Some(1)),
            (Function::CurrentTimestamp, 0, Some(0)),
            (Function::Contains, 1, Some(1)),
        ] {
            assert_eq!(function.arity(), Arity { min, max }, "{}", function);
        }
    }

    #[test]
    fn argument_count_errors_name_the_function_and_the_expectation() {
        assert_eq!(Function::Lower.check_argument_count(2).unwrap_err(), "Function LOWER expects 1 argument, got 2");
        assert_eq!(Function::Pi.check_argument_count(1).unwrap_err(), "Function PI expects no arguments, got 1");
        assert_eq!(Function::Coalesce.check_argument_count(0).unwrap_err(), "Function COALESCE expects at least 1 argument, got 0");
        assert_eq!(Function::Substring.check_argument_count(4).unwrap_err(), "Function SUBSTRING expects 2 to 3 arguments, got 4");
        assert_eq!(Function::Replace.check_argument_count(2).unwrap_err(), "Function REPLACE expects 3 arguments, got 2");
        assert!(Function::Substring.check_argument_count(2).is_ok());
        assert!(Function::Coalesce.check_argument_count(5).is_ok());
        assert!(Function::Count.check_argument_count(0).is_ok());
    }

    #[test]
    fn strpos_takes_the_haystack_first() {
        assert_eq!(Function::from_str("strpos"), Ok(Function::Strpos));

        let strpos = |haystack: &str, rest: &[&str]| {
            get_value(&Function::Strpos, String::from(haystack), rest.iter().map(|s| s.to_string()).collect(), None, &None)
        };
        assert_eq!(strpos("hello world", &["world"]).unwrap().to_int(), 7);
        assert_eq!(strpos("hello world", &["o", "6"]).unwrap().to_int(), 8);
        assert_eq!(strpos("hello world", &["xyz"]).unwrap().to_int(), 0);
        assert_eq!(strpos("aéb", &["b"]).unwrap().to_int(), 3);
        assert!(strpos("hello", &[]).is_err());
        assert!(strpos("hello", &["l", "abc"]).is_err());
    }

    #[test]
    fn locate_and_strpos_are_mirror_images() {
        let locate = get_value(&Function::Locate, String::from("o"), vec![String::from("hello")], None, &None);
        let strpos = get_value(&Function::Strpos, String::from("hello"), vec![String::from("o")], None, &None);
        assert_eq!(locate.unwrap().to_int(), 5);
        assert_eq!(strpos.unwrap().to_int(), 5);

        // The documented MySQL example: the substring comes first.
        let documented = get_value(&Function::Locate, String::from("foo"), vec![String::from("barfoo")], None, &None);
        assert_eq!(documented.unwrap().to_int(), 4);
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
    fn function_substring_negative_length() {
        let function = Function::Substring;
        let function_arg = String::from("hello world");
        let function_args = vec![String::from("7"), String::from("-1")];
        let entry = None;
        let file_info = None;

        // MySQL returns an empty string for a non-positive length, not an error
        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "");
    }

    #[test]
    fn function_substring_zero_length() {
        let function = Function::Substring;
        let function_arg = String::from("hello world");
        let function_args = vec![String::from("7"), String::from("0")];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "");
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
    fn function_log_is_exact_for_powers_of_its_base() {
        let log = |arg: &str, args: &[&str]| {
            let args = args.iter().map(|a| a.to_string()).collect();
            get_value(&Function::Log, arg.to_string(), args, None, &None).unwrap().to_float()
        };
        assert_eq!(log("1000", &[]), 3.0);
        assert_eq!(log("1000", &["10"]), 3.0);
        assert_eq!(log("1e15", &[]), 15.0);
        assert_eq!(log("0.001", &[]), -3.0);
        assert_eq!(log("1024", &["2"]), 10.0);
        assert_eq!(log("0.125", &["2"]), -3.0);
        assert_eq!(log("27", &["3"]), 3.0);
        assert!((log("20", &["4"]) - 20f64.ln() / 4f64.ln()).abs() < 1e-15);
    }

    #[test]
    fn function_format_size_accepts_fractional_and_suffixed_input() {
        let format_size = |arg: &str| {
            get_value(&Function::FormatSize, arg.to_string(), vec![], None, &None).unwrap().to_string()
        };
        assert_eq!(format_size("1536"), "1.50KiB");
        assert_eq!(format_size("1536.4"), "1.50KiB");
        assert_eq!(format_size("4.333333333333333"), "4B");
        assert_eq!(format_size("0.5"), "1B");
        assert_eq!(format_size("1kib"), "1KiB");
        assert_eq!(format_size("2 MiB"), "2MiB");
        assert_eq!(format_size("1mb"), "976.56KiB");
        assert_eq!(format_size("-5"), "");
        assert_eq!(format_size("abc"), "");
        assert_eq!(format_size(""), "");
    }

    #[test]
    fn function_format_size_rejects_an_unknown_modifier() {
        let result = get_value(&Function::FormatSize, String::from("1536000"), vec![String::from("short")], None, &None);
        match result {
            Err(err) => assert!(err.contains("Invalid file size format modifier"), "{}", err),
            Ok(value) => panic!("an unknown modifier must be an error, got {:?}", value.to_string()),
        }
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
    fn least_and_greatest_compare_strings_lexicographically() {
        let least = get_value(&Function::Least, String::from("pear"), vec![String::from("apple"), String::from("fig")], None, &None);
        assert_eq!(least.unwrap().to_string(), "apple");
        let greatest = get_value(&Function::Greatest, String::from("pear"), vec![String::from("apple"), String::from("fig")], None, &None);
        assert_eq!(greatest.unwrap().to_string(), "pear");

        // One non-numeric argument turns the whole comparison into a string one, as in MySQL.
        let mixed = get_value(&Function::Greatest, String::from("10"), vec![String::from("9"), String::from("abc")], None, &None);
        assert_eq!(mixed.unwrap().to_string(), "abc");
        let mixed = get_value(&Function::Least, String::from("10"), vec![String::from("9"), String::from("abc")], None, &None);
        assert_eq!(mixed.unwrap().to_string(), "10");
    }

    #[test]
    fn least_and_greatest_skip_empty_arguments_before_choosing_the_comparison() {
        // An empty (missing) value must not demote `10` vs `9` to a string comparison.
        let least = get_value(&Function::Least, String::from("10"), vec![String::from(""), String::from("9")], None, &None);
        assert_eq!(least.unwrap().to_int(), 9);
        let only_empty = get_value(&Function::Greatest, String::from(""), vec![String::from("")], None, &None);
        assert_eq!(only_empty.unwrap().to_string(), "");
        let strings = get_value(&Function::Greatest, String::from(""), vec![String::from("b"), String::from("a")], None, &None);
        assert_eq!(strings.unwrap().to_string(), "b");
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
        assert!(result.unwrap().to_bool());
    }
    
    #[test]
    fn function_contains_hiragana() {
        let function = Function::ContainsHiragana;
        let function_arg = String::from("こんにちは");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert!(result.unwrap().to_bool());
    }
    
    #[test]
    fn function_contains_katakana() {
        let function = Function::ContainsKatakana;
        let function_arg = String::from("カタカナ");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert!(result.unwrap().to_bool());
    }
    
    #[test]
    fn function_contains_kana() {
        let function = Function::ContainsKana;
        let function_arg = String::from("カタカナ");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert!(result.unwrap().to_bool());
    }
    
    #[test]
    fn function_contains_kanji() {
        let function = Function::ContainsKanji;
        let function_arg = String::from("漢字");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert!(result.unwrap().to_bool());
    }
    
    #[test]
    fn function_contains_greek() {
        let function = Function::ContainsGreek;
        let function_arg = String::from("Ελληνικά");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert!(result.unwrap().to_bool());
    }
    
    #[test]
    fn functions_reading_the_entry_are_marked_in_metadata() {
        assert!(Function::Contains.reads_entry());
        #[cfg(any(unix, windows))]
        {
            assert!(Function::HasXattr.reads_entry());
            assert!(Function::Xattr.reads_entry());
        }
        #[cfg(any(target_os = "linux", windows))]
        {
            assert!(Function::HasExtattr.reads_entry());
            assert!(Function::HasAclEntry.reads_entry());
            assert!(Function::AclEntry.reads_entry());
            assert!(Function::HasDefaultAclEntry.reads_entry());
            assert!(Function::DefaultAclEntry.reads_entry());
        }
        #[cfg(target_os = "linux")]
        assert!(Function::HasCapability.reads_entry());
        for function in [Function::Upper, Function::Length, Function::Count, Function::Year, Function::Random] {
            assert!(!function.reads_entry(), "{}", function);
        }
    }

    #[test]
    fn constant_functions_have_no_weight() {
        let constants = vec![
            Function::Pi, Function::CurrentDate, Function::CurrentTime, Function::CurrentTimestamp, Function::Random,
            #[cfg(all(unix, feature = "users"))]
            Function::CurrentUid,
            #[cfg(all(unix, feature = "users"))]
            Function::CurrentUser,
            #[cfg(all(unix, feature = "users"))]
            Function::CurrentGid,
            #[cfg(all(unix, feature = "users"))]
            Function::CurrentGroup,
        ];
        for function in constants {
            assert_eq!(function.get_weight(), 0, "{}", function);
        }
        assert_eq!(Function::Contains.get_weight(), 1024);
    }

    #[test]
    fn function_contains_cyrillic() {
        for (arg, expected) in [("Привет", true), ("Ελληνικά", false), ("hello", false), ("Ёлка.txt", true)] {
            let result = get_value(&Function::ContainsCyrillic, String::from(arg), vec![], None, &None);
            assert_eq!(result.unwrap().to_bool(), expected, "{}", arg);
        }
        assert_eq!(Function::from_str("cyrillic").ok(), Some(Function::ContainsCyrillic));
        assert_eq!(Function::from_str("contains_cyrillic").ok(), Some(Function::ContainsCyrillic));
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
        let format_time = |arg: &str| {
            get_value(&Function::FormatTime, String::from(arg), vec![], None, &None)
                .unwrap()
                .to_string()
        };
        assert_eq!(format_time("146"), "2m 26s");
        assert_eq!(format_time("1.5"), "1s 500ms");
        assert_eq!(format_time("abc"), "");
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
    fn function_dayname_sunday() {
        let function = Function::DayName;
        let function_arg = String::from("2023-10-01"); // Sunday
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "Sunday");
    }

    #[test]
    fn function_dayname_wednesday() {
        let function = Function::DayName;
        let function_arg = String::from("2023-10-04"); // Wednesday
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "Wednesday");
    }

    #[test]
    fn function_dayname_invalid() {
        let function = Function::DayName;
        let function_arg = String::from("not-a-date");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "");
    }

    #[test]
    fn function_dayofyear() {
        let function = Function::DayOfYear;
        let function_arg = String::from("2023-03-01"); // 60th day
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_int(), 60);
    }

    #[test]
    fn function_dayofyear_jan1() {
        let function = Function::DayOfYear;
        let function_arg = String::from("2023-01-01");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_int(), 1);
    }

    #[test]
    fn function_dayofyear_dec31() {
        let function = Function::DayOfYear;
        let function_arg = String::from("2023-12-31");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_int(), 365);
    }

    #[test]
    fn function_date_add() {
        let function = Function::DateAdd;
        let function_arg = String::from("2023-10-01");
        let function_args = vec![String::from("10")];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert!(result.unwrap().to_string().starts_with("2023-10-11"));
    }

    #[test]
    fn function_date_add_negative() {
        let function = Function::DateAdd;
        let function_arg = String::from("2023-10-01");
        let function_args = vec![String::from("-5")];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert!(result.unwrap().to_string().starts_with("2023-09-26"));
    }

    #[test]
    fn function_date_add_missing_arg() {
        let function = Function::DateAdd;
        let function_arg = String::from("2023-10-01");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert!(result.is_err());
    }

    #[test]
    fn function_date_sub() {
        let function = Function::DateSub;
        let function_arg = String::from("2023-10-15");
        let function_args = vec![String::from("10")];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert!(result.unwrap().to_string().starts_with("2023-10-05"));
    }

    #[test]
    fn function_date_sub_cross_month() {
        let function = Function::DateSub;
        let function_arg = String::from("2023-03-01");
        let function_args = vec![String::from("1")];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert!(result.unwrap().to_string().starts_with("2023-02-28"));
    }

    #[test]
    fn function_date_sub_missing_arg() {
        let function = Function::DateSub;
        let function_arg = String::from("2023-10-01");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert!(result.is_err());
    }

    #[test]
    fn function_date_diff() {
        let function = Function::DateDiff;
        let function_arg = String::from("2023-10-15");
        let function_args = vec![String::from("2023-10-01")];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_int(), 14);
    }

    #[test]
    fn function_date_diff_negative() {
        let function = Function::DateDiff;
        let function_arg = String::from("2023-10-01");
        let function_args = vec![String::from("2023-10-15")];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_int(), -14);
    }

    #[test]
    fn function_date_diff_same_day() {
        let function = Function::DateDiff;
        let function_arg = String::from("2023-10-01");
        let function_args = vec![String::from("2023-10-01")];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_int(), 0);
    }

    #[test]
    fn function_date_diff_ignores_time_parts() {
        let function = Function::DateDiff;
        let function_arg = String::from("2024-01-02");
        let function_args = vec![String::from("2024-01-01 23:59:59")];
        let entry = None;
        let file_info = None;

        // MySQL DATEDIFF diffs calendar dates, not full 24-hour periods
        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_int(), 1);
    }

    #[test]
    fn function_date_diff_missing_arg() {
        let function = Function::DateDiff;
        let function_arg = String::from("2023-10-01");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert!(result.is_err());
    }

    #[test]
    fn function_from_unixtime() {
        let function = Function::FromUnixtime;
        let function_arg = String::from("0");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        // Rendered as local time, whatever the machine timezone is
        let expected = DateTime::from_timestamp(0, 0)
            .unwrap()
            .with_timezone(&Local)
            .naive_local();

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), format_datetime(&expected));
    }

    #[test]
    fn function_from_unixtime_specific() {
        let function = Function::FromUnixtime;
        let function_arg = String::from("1696118400"); // 2023-10-01 00:00:00 UTC
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let expected = DateTime::from_timestamp(1696118400, 0)
            .unwrap()
            .with_timezone(&Local)
            .naive_local();

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), format_datetime(&expected));
    }

    #[test]
    fn function_from_unixtime_extract_epoch_roundtrip() {
        // extract('epoch', from_unixtime(T)) must give T back regardless of timezone
        let timestamp = 1696118400_i64; // 2023-10-01 00:00:00 UTC, no DST edge nearby
        let formatted = get_value(
            &Function::FromUnixtime,
            timestamp.to_string(),
            vec![],
            None,
            &None,
        )
        .unwrap()
        .to_string();

        let result = get_value(
            &Function::Extract,
            String::from("epoch"),
            vec![formatted],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), timestamp);
    }

    #[test]
    fn function_from_unixtime_invalid() {
        let function = Function::FromUnixtime;
        let function_arg = String::from("not-a-number");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "");
    }

    #[test]
    fn function_last_day_october() {
        let function = Function::LastDay;
        let function_arg = String::from("2023-10-15");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "2023-10-31");
    }

    #[test]
    fn function_last_day_february() {
        let function = Function::LastDay;
        let function_arg = String::from("2023-02-01");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "2023-02-28");
    }

    #[test]
    fn function_last_day_february_leap() {
        let function = Function::LastDay;
        let function_arg = String::from("2024-02-10");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "2024-02-29");
    }

    #[test]
    fn function_last_day_december() {
        let function = Function::LastDay;
        let function_arg = String::from("2023-12-05");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "2023-12-31");
    }

    #[test]
    fn function_last_day_invalid() {
        let function = Function::LastDay;
        let function_arg = String::from("not-a-date");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert_eq!(result.unwrap().to_string(), "");
    }

    #[test]
    fn function_last_day_min_date() {
        let function = Function::LastDay;
        let function_arg = String::from("0001-01-01");
        let function_args = vec![];
        let entry = None;
        let file_info = None;

        let result = get_value(&function, function_arg, function_args, entry, &file_info);
        assert!(result.is_ok(), "LAST_DAY should not panic on edge dates");
    }

    #[test]
    fn function_extract_year() {
        let result = get_value(
            &Function::Extract,
            String::from("year"),
            vec![String::from("2023-10-15 12:34:56")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), 2023);
    }

    #[test]
    fn function_extract_month_uppercase() {
        let result = get_value(
            &Function::Extract,
            String::from("MONTH"),
            vec![String::from("2023-10-15")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), 10);
    }

    #[test]
    fn function_extract_day() {
        let result = get_value(
            &Function::Extract,
            String::from("day"),
            vec![String::from("2023-10-15")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), 15);
    }

    #[test]
    fn function_extract_hour() {
        let result = get_value(
            &Function::Extract,
            String::from("hour"),
            vec![String::from("2023-10-15 12:34:56")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), 12);
    }

    #[test]
    fn function_extract_minute() {
        let result = get_value(
            &Function::Extract,
            String::from("minute"),
            vec![String::from("2023-10-15 12:34:56")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), 34);
    }

    #[test]
    fn function_extract_second() {
        let result = get_value(
            &Function::Extract,
            String::from("second"),
            vec![String::from("2023-10-15 12:34:56")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), 56);
    }

    #[test]
    fn function_extract_quarter() {
        // October falls in Q4
        let result = get_value(
            &Function::Extract,
            String::from("quarter"),
            vec![String::from("2023-10-15")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), 4);
        // March falls in Q1
        let result = get_value(
            &Function::Extract,
            String::from("quarter"),
            vec![String::from("2023-03-31")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), 1);
    }

    #[test]
    fn function_extract_week() {
        // 2023-01-02 (Monday) is ISO week 1
        let result = get_value(
            &Function::Extract,
            String::from("week"),
            vec![String::from("2023-01-02")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), 1);
    }

    #[test]
    fn function_extract_doy() {
        let result = get_value(
            &Function::Extract,
            String::from("doy"),
            vec![String::from("2023-12-31")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), 365);
    }

    #[test]
    fn function_extract_epoch() {
        // The naive datetime is interpreted as local time
        let expected = NaiveDate::from_ymd_opt(2023, 10, 1)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_local_timezone(Local)
            .single()
            .unwrap()
            .timestamp();

        let result = get_value(
            &Function::Extract,
            String::from("epoch"),
            vec![String::from("2023-10-01 12:00:00")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), expected);
    }

    #[test]
    fn function_extract_invalid_unit() {
        let result = get_value(
            &Function::Extract,
            String::from("century"),
            vec![String::from("2023-10-15")],
            None,
            &None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn function_extract_invalid_date() {
        let result = get_value(
            &Function::Extract,
            String::from("year"),
            vec![String::from("not-a-date")],
            None,
            &None,
        );
        // Bad date yields empty Int, not an error (matches existing YEAR/MONTH behavior).
        assert_eq!(result.unwrap().to_string(), "");
    }

    #[test]
    fn function_extract_missing_arg() {
        let result = get_value(
            &Function::Extract,
            String::from("year"),
            vec![],
            None,
            &None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn function_date_trunc_year() {
        let result = get_value(
            &Function::DateTrunc,
            String::from("year"),
            vec![String::from("2023-10-15 12:34:56")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "2023-01-01 00:00:00");
    }

    #[test]
    fn function_date_trunc_quarter() {
        // October → Q4 starts in October
        let result = get_value(
            &Function::DateTrunc,
            String::from("quarter"),
            vec![String::from("2023-11-15 12:34:56")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "2023-10-01 00:00:00");
        // May → Q2 starts in April
        let result = get_value(
            &Function::DateTrunc,
            String::from("quarter"),
            vec![String::from("2023-05-15")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "2023-04-01 00:00:00");
    }

    #[test]
    fn function_date_trunc_month() {
        let result = get_value(
            &Function::DateTrunc,
            String::from("month"),
            vec![String::from("2023-10-15 12:34:56")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "2023-10-01 00:00:00");
    }

    #[test]
    fn function_date_trunc_week() {
        // 2023-10-15 is a Sunday → previous Monday is 2023-10-09
        let result = get_value(
            &Function::DateTrunc,
            String::from("week"),
            vec![String::from("2023-10-15 12:34:56")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "2023-10-09 00:00:00");
    }

    #[test]
    fn function_date_trunc_day() {
        let result = get_value(
            &Function::DateTrunc,
            String::from("day"),
            vec![String::from("2023-10-15 12:34:56")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "2023-10-15 00:00:00");
    }

    #[test]
    fn function_date_trunc_hour() {
        let result = get_value(
            &Function::DateTrunc,
            String::from("hour"),
            vec![String::from("2023-10-15 12:34:56")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "2023-10-15 12:00:00");
    }

    #[test]
    fn function_date_trunc_minute() {
        let result = get_value(
            &Function::DateTrunc,
            String::from("minute"),
            vec![String::from("2023-10-15 12:34:56")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "2023-10-15 12:34:00");
    }

    #[test]
    fn function_date_trunc_invalid_unit() {
        let result = get_value(
            &Function::DateTrunc,
            String::from("decade"),
            vec![String::from("2023-10-15")],
            None,
            &None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn function_date_trunc_invalid_date() {
        let result = get_value(
            &Function::DateTrunc,
            String::from("year"),
            vec![String::from("not-a-date")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "");
    }

    #[test]
    fn function_date_trunc_missing_arg() {
        let result = get_value(
            &Function::DateTrunc,
            String::from("year"),
            vec![],
            None,
            &None,
        );
        assert!(result.is_err());
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
            String::from("b"),
            vec![String::from("aéb")],
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
    fn min_max_compare_formatted_sizes_numerically() {
        let acc = make_accumulator("fsize", &["11.72KiB", "1.91MiB", "900B"]);
        assert_eq!(get_aggregate_value(&Function::Min, &acc, "fsize".to_string(), &None), "900B");
        assert_eq!(get_aggregate_value(&Function::Max, &acc, "fsize".to_string(), &None), "1.91MiB");

        let acc = make_accumulator("fsize", &["2 KB", "900 B", "1 MB"]);
        assert_eq!(get_aggregate_value(&Function::Min, &acc, "fsize".to_string(), &None), "900 B");
        assert_eq!(get_aggregate_value(&Function::Max, &acc, "fsize".to_string(), &None), "1 MB");
    }

    #[test]
    fn min_and_max_over_mixed_text_keep_every_value() {
        let mixed = make_accumulator("val", &["10", "9", "abc", "zeta", "Beta", "2x", ""]);
        assert_eq!(get_aggregate_value(&Function::Min, &mixed, "val".to_string(), &None), "10");
        assert_eq!(get_aggregate_value(&Function::Max, &mixed, "val".to_string(), &None), "zeta");

        let numeric = make_accumulator("val", &["10", "9", "", "100"]);
        assert_eq!(get_aggregate_value(&Function::Min, &numeric, "val".to_string(), &None), "9");
        assert_eq!(get_aggregate_value(&Function::Max, &numeric, "val".to_string(), &None), "100");
    }

    #[test]
    fn min_and_max_keep_the_spelling_of_numeric_looking_text() {
        let padded = make_accumulator("val", &["002", "010", "1e3"]);
        assert_eq!(get_aggregate_value(&Function::Min, &padded, "val".to_string(), &None), "002");
        assert_eq!(get_aggregate_value(&Function::Max, &padded, "val".to_string(), &None), "1e3");

        let plain = make_accumulator("val", &["10", "9", "100"]);
        assert_eq!(get_aggregate_value(&Function::Min, &plain, "val".to_string(), &None), "9");
        assert_eq!(get_aggregate_value(&Function::Max, &plain, "val".to_string(), &None), "100");
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
    fn min_max_over_datetime_values() {
        let acc = make_accumulator(
            "Modified",
            &[
                "2026-06-11 10:00:00",
                "2024-01-05 09:30:00",
                "2025-12-31 23:59:59",
            ],
        );
        assert_eq!(
            get_aggregate_value(&Function::Min, &acc, "Modified".to_string(), &None),
            "2024-01-05 09:30:00"
        );
        assert_eq!(
            get_aggregate_value(&Function::Max, &acc, "Modified".to_string(), &None),
            "2026-06-11 10:00:00"
        );
    }

    #[test]
    fn min_max_over_string_values() {
        let acc = make_accumulator("Name", &["banana", "apple", "cherry"]);
        assert_eq!(get_aggregate_value(&Function::Min, &acc, "Name".to_string(), &None), "apple");
        assert_eq!(get_aggregate_value(&Function::Max, &acc, "Name".to_string(), &None), "cherry");
    }

    #[test]
    fn min_max_skip_empty_values() {
        let acc = make_accumulator("Name", &["", "b", "a", ""]);
        assert_eq!(get_aggregate_value(&Function::Min, &acc, "Name".to_string(), &None), "a");
        assert_eq!(get_aggregate_value(&Function::Max, &acc, "Name".to_string(), &None), "b");
    }

    #[test]
    fn min_max_only_empty_values_returns_empty() {
        let acc = make_accumulator("Width", &["", ""]);
        assert_eq!(get_aggregate_value(&Function::Min, &acc, "Width".to_string(), &None), "");
        assert_eq!(get_aggregate_value(&Function::Max, &acc, "Width".to_string(), &None), "");
    }

    #[test]
    fn min_max_over_mixed_values_use_text_order() {
        // A column mixing numeric and non-numeric values is a text column:
        // every value takes part, in the same order ORDER BY would use.
        let acc = make_accumulator("val", &["10", "9", "abc"]);
        assert_eq!(get_aggregate_value(&Function::Min, &acc, "val".to_string(), &None), "10");
        assert_eq!(get_aggregate_value(&Function::Max, &acc, "val".to_string(), &None), "abc");
    }

    #[test]
    fn count_star_counts_all_rows() {
        let acc = make_accumulator("Width", &["100", "", "200"]);
        assert_eq!(get_aggregate_value(&Function::Count, &acc, "*".to_string(), &None), "3");
        // Bare COUNT (no argument) behaves like COUNT(*)
        assert_eq!(get_aggregate_value(&Function::Count, &acc, String::new(), &None), "3");
    }

    #[test]
    fn count_column_skips_empty_values() {
        let acc = make_accumulator("Width", &["100", "", "200"]);
        assert_eq!(get_aggregate_value(&Function::Count, &acc, "Width".to_string(), &None), "2");
    }

    #[test]
    fn count_column_includes_non_numeric_values() {
        let acc = make_accumulator("Name", &["a.txt", "b.txt", ""]);
        assert_eq!(get_aggregate_value(&Function::Count, &acc, "Name".to_string(), &None), "2");
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
    fn random_inverted_range_returns_value_in_range() {
        let result = get_value(
            &Function::Random,
            String::from("5"),
            vec![String::from("3")],
            None,
            &None,
        );
        let val = result.unwrap().to_int();
        assert!((3..=5).contains(&val), "RANDOM(5,3) should return value in [3,5], got {}", val);
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
            String::from("l"),
            vec![String::from("hello"), String::from("abc")],
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
    fn sqrt_negative_returns_empty() {
        let result = get_value(
            &Function::Sqrt,
            String::from("-1"),
            vec![],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "");
    }

    #[test]
    fn ln_zero_returns_empty() {
        let result = get_value(
            &Function::Ln,
            String::from("0"),
            vec![],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "");
    }

    #[test]
    fn ln_negative_returns_empty() {
        let result = get_value(
            &Function::Ln,
            String::from("-1"),
            vec![],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "");
    }

    #[test]
    fn log_base_one_returns_empty() {
        let result = get_value(
            &Function::Log,
            String::from("100"),
            vec![String::from("1")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "");
    }

    #[test]
    fn log_nan_base_returns_empty() {
        let result = get_value(
            &Function::Log,
            String::from("100"),
            vec![String::from("NaN")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "");
    }

    #[test]
    fn log_infinity_base_returns_empty() {
        let result = get_value(
            &Function::Log,
            String::from("100"),
            vec![String::from("inf")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "");
    }

    #[test]
    fn log_negative_value_returns_empty() {
        let result = get_value(
            &Function::Log,
            String::from("-1"),
            vec![String::from("10")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "");
    }

    #[test]
    fn power_negative_base_fractional_exp_returns_empty() {
        let result = get_value(
            &Function::Power,
            String::from("-2"),
            vec![String::from("0.5")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "");
    }

    #[test]
    fn power_zero_base_negative_exp_returns_empty() {
        let result = get_value(
            &Function::Power,
            String::from("0"),
            vec![String::from("-1")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "");
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
    fn exp_overflow_returns_empty() {
        let result = get_value(
            &Function::Exp,
            String::from("710"),
            vec![],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_string(), "");
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
            String::from("h"),
            vec![String::from("hello"), String::from("0")],
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
    fn min_no_parseable_values_falls_back_to_lexicographic() {
        let acc = make_accumulator("val", &["abc", "def"]);
        let result = get_aggregate_value(&Function::Min, &acc, String::from("val"), &None);
        assert_eq!(result, "abc");
    }

    #[test]
    fn max_no_parseable_values_falls_back_to_lexicographic() {
        let acc = make_accumulator("val", &["abc", "def"]);
        let result = get_aggregate_value(&Function::Max, &acc, String::from("val"), &None);
        assert_eq!(result, "def");
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
            assert!((0..=1).contains(&val));
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
            assert!((5..=6).contains(&val));
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
    fn min_and_max_keep_all_zero_spellings() {
        let names = make_accumulator("val", &["000", "001", "002"]);
        assert_eq!(get_aggregate_value(&Function::Min, &names, "val".to_string(), &None), "000");
        assert_eq!(get_aggregate_value(&Function::Max, &names, "val".to_string(), &None), "002");

        let decimal = make_accumulator("val", &["0.0", "0.5"]);
        assert_eq!(get_aggregate_value(&Function::Min, &decimal, "val".to_string(), &None), "0.0");

        let negative = make_accumulator("val", &["-0.0", "1"]);
        assert_eq!(get_aggregate_value(&Function::Min, &negative, "val".to_string(), &None), "-0.0");

        let signed_names = make_accumulator("val", &["-000", "001"]);
        assert_eq!(get_aggregate_value(&Function::Min, &signed_names, "val".to_string(), &None), "-000");
    }

    #[test]
    fn min_negative_zero_keeps_the_first_spelling() {
        let acc = make_accumulator("val", &["-0", "0"]);
        let result = get_aggregate_value(&Function::Min, &acc, "val".to_string(), &None);
        assert_eq!(result, "-0");
        let acc = make_accumulator("val", &["0", "-0"]);
        let result = get_aggregate_value(&Function::Min, &acc, "val".to_string(), &None);
        assert_eq!(result, "0");
    }

    #[test]
    fn max_negative_zero_keeps_the_first_spelling() {
        let acc = make_accumulator("val", &["-0", "0"]);
        let result = get_aggregate_value(&Function::Max, &acc, "val".to_string(), &None);
        assert_eq!(result, "-0");
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
            String::from(""),
            vec![String::from("hello"), String::from("100")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), 0);
    }

    #[test]
    fn locate_beyond_string_length_returns_zero() {
        let result = get_value(
            &Function::Locate,
            String::from("o"),
            vec![String::from("hello"), String::from("100")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), 0);
    }

    #[test]
    fn locate_empty_substring_at_end_plus_one() {
        let result = get_value(
            &Function::Locate,
            String::from(""),
            vec![String::from("hello"), String::from("6")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), 6);
    }

    #[test]
    fn locate_empty_substring_past_end_plus_one() {
        let result = get_value(
            &Function::Locate,
            String::from(""),
            vec![String::from("hello"), String::from("7")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), 0);
    }

    #[test]
    fn greek_string_group_in_help() {
        let groups = Function::get_groups();
        assert!(groups.contains(&"Greek string"));
        let descs = Function::get_names_and_descriptions();
        assert!(descs.contains_key("Greek string"));
    }

    #[test]
    fn cyrillic_string_group_in_help() {
        let groups = Function::get_groups();
        let greek = groups.iter().position(|g| *g == "Greek string").unwrap();
        assert_eq!(groups.get(greek + 1), Some(&"Cyrillic string"));
        let descs = Function::get_names_and_descriptions();
        assert!(descs.get("Cyrillic string").is_some_and(|names| names.iter().any(|(n, _)| n.contains(&"cyrillic"))));
    }

    #[test]
    fn locate_i32_min_position_no_overflow() {
        let result = get_value(
            &Function::Locate,
            String::from("l"),
            vec![String::from("hello"), String::from("-2147483648")],
            None,
            &None,
        );
        assert_eq!(result.unwrap().to_int(), 3);
    }

    #[test]
    fn date_add_extreme_days_no_panic() {
        let result = get_value(
            &Function::DateAdd,
            String::from("2024-01-01"),
            vec![String::from("999999999999999")],
            None,
            &None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn date_sub_extreme_days_no_panic() {
        let result = get_value(
            &Function::DateSub,
            String::from("2024-01-01"),
            vec![String::from("999999999999999")],
            None,
            &None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn date_add_and_sub_beyond_the_calendar_report_an_error() {
        for function in [Function::DateAdd, Function::DateSub] {
            for days in ["100000000", "-100000000", "106751991167300"] {
                let result = get_value(&function, String::from("2024-01-01"), vec![String::from(days)], None, &None);
                assert_eq!(
                    result,
                    Err(format!("Number of days out of range: {}", days)),
                    "{} {}",
                    function,
                    days
                );
            }
        }
    }

    fn write_temp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        use std::io::Write;
        let dir = std::env::temp_dir().join("fselect_contains_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(bytes).unwrap();
        path
    }

    fn dir_entry_for(path: &std::path::Path) -> std::fs::DirEntry {
        let parent = path.parent().unwrap();
        let target = path.file_name().unwrap();
        std::fs::read_dir(parent)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_name() == target)
            .unwrap()
    }

    #[test]
    fn contains_finds_substring_in_non_utf8_file() {
        // A file whose body is mostly invalid UTF-8 but contains the literal
        // ASCII substring "needle". The streaming implementation must find it;
        // a String-based read_to_string fails outright on invalid UTF-8 and
        // wrongly reports the substring as absent.
        let mut bytes: Vec<u8> = vec![0xfd, 0xfc, 0xff, 0xfe, 0x80, 0x81];
        bytes.extend_from_slice(b"needle");
        bytes.extend_from_slice(&[0xff, 0xfe, 0xfd]);
        let path = write_temp_file("non_utf8.bin", &bytes);
        let entry = dir_entry_for(&path);

        let result = get_value(
            &Function::Contains,
            String::from("needle"),
            vec![],
            Some(&entry),
            &None,
        )
        .unwrap();
        assert!(result.to_bool(), "Contains should find substring even in non-UTF8 files");

        let _ = std::fs::remove_file(&path);
    }

    fn contains_in(name: &str, bytes: &[u8], needle: &str) -> bool {
        let path = write_temp_file(name, bytes);
        let entry = dir_entry_for(&path);
        let result = get_value(&Function::Contains, String::from(needle), vec![], Some(&entry), &None).unwrap();
        let _ = std::fs::remove_file(&path);
        result.to_bool()
    }

    fn encode_text(text: &str, encoding: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        match encoding {
            "UTF-16LE" => {
                bytes.extend_from_slice(&[0xFF, 0xFE]);
                for unit in text.encode_utf16() {
                    bytes.extend_from_slice(&unit.to_le_bytes());
                }
            }
            "UTF-16BE" => {
                bytes.extend_from_slice(&[0xFE, 0xFF]);
                for unit in text.encode_utf16() {
                    bytes.extend_from_slice(&unit.to_be_bytes());
                }
            }
            "UTF-32LE" => {
                bytes.extend_from_slice(&[0xFF, 0xFE, 0x00, 0x00]);
                for c in text.chars() {
                    bytes.extend_from_slice(&(c as u32).to_le_bytes());
                }
            }
            "UTF-32BE" => {
                bytes.extend_from_slice(&[0x00, 0x00, 0xFE, 0xFF]);
                for c in text.chars() {
                    bytes.extend_from_slice(&(c as u32).to_be_bytes());
                }
            }
            _ => bytes.extend_from_slice(text.as_bytes()),
        }
        bytes
    }

    #[test]
    fn contains_searches_text_in_its_recognized_encoding() {
        for encoding in ["UTF-16LE", "UTF-16BE", "UTF-32LE", "UTF-32BE"] {
            let bytes = encode_text("hello world", encoding);
            assert!(contains_in(&format!("hello_{encoding}.txt"), &bytes, "hello"), "{encoding}: hello");
            assert!(contains_in(&format!("world_{encoding}.txt"), &bytes, "o w"), "{encoding}: o w");
            assert!(!contains_in(&format!("absent_{encoding}.txt"), &bytes, "bye"), "{encoding}: bye");
        }
        let kanji = encode_text("\u{4241}", "UTF-16LE");
        assert!(!contains_in("kanji_utf16le.txt", &kanji, "AB"), "UTF-16LE bytes 41 42 are not the text AB");
        assert!(contains_in("kanji_utf16le_self.txt", &kanji, "\u{4241}"), "the character itself is found");
        let utf8_bom = encode_text("hello world", "UTF-8");
        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend_from_slice(&utf8_bom);
        assert!(contains_in("hello_utf8_bom.txt", &with_bom, "hello"), "UTF-8 with BOM");
        // 上 is 0A 4E and 乎 is 4E 4E in UTF-16LE: the bytes 4E 4E first occur
        // unaligned at offset 1, and the aligned occurrence at offset 2
        // overlaps it.
        for encoding in ["UTF-16LE", "UTF-16BE"] {
            let overlapping = encode_text("\u{4E0A}\u{4E4E}", encoding);
            assert!(contains_in(&format!("overlap_{encoding}.txt"), &overlapping, "\u{4E4E}"), "{encoding}: overlapping aligned match");
        }
    }

    #[test]
    fn contains_searches_latin1_text_as_latin1() {
        let latin1_cafe = b"un caf\xe9 noir";
        assert!(contains_in("latin1_cafe.txt", latin1_cafe, "caf\u{e9}"), "Latin-1 caf\u{e9}");
        assert!(contains_in("latin1_ascii.txt", latin1_cafe, "noir"), "ASCII needle in a Latin-1 file");
        assert!(!contains_in("latin1_absent.txt", latin1_cafe, "th\u{e9}"), "absent needle");
        // Invalid UTF-8 overall (E9 20), so the body is Latin-1 text
        // "\u{e9} caf\u{c3}\u{a9}": the UTF-8 bytes of caf\u{e9} appear, the text does not.
        let latin1_lookalike = b"\xe9 caf\xc3\xa9";
        assert!(!contains_in("latin1_lookalike.txt", latin1_lookalike, "caf\u{e9}"), "UTF-8 bytes inside Latin-1 text are not a match");
        assert!(contains_in("utf8_cafe.txt", "un caf\u{e9} noir".as_bytes(), "caf\u{e9}"), "UTF-8 caf\u{e9}");
        // A Latin-1 body ending in a non-ASCII byte leaves a lone lead byte
        // at the end of the file: an incomplete sequence, not valid UTF-8.
        assert!(contains_in("latin1_trailing.txt", b"caf\xe9", "caf\u{e9}"), "Latin-1 caf\u{e9} at the end of the file");
        assert!(contains_in("latin1_single.txt", b"\xe9", "\u{e9}"), "a single Latin-1 byte");
        assert!(contains_in("latin1_trailing_lead.txt", b"caf\xc3", "caf\u{c3}"), "a trailing byte that is a UTF-8 lead byte");
        // A NUL anywhere makes the body binary, even chunks after a Latin-1
        // match: binary bodies are searched as raw bytes only.
        let mut late_nul = b"caf\xe9 ".to_vec();
        late_nul.extend(std::iter::repeat_n(b'x', 16_000));
        late_nul.push(0);
        assert!(!contains_in("latin1_late_nul.bin", &late_nul, "caf\u{e9}"), "a NUL in a later chunk makes the body binary");
        let mut no_nul = b"caf\xe9 ".to_vec();
        no_nul.extend(std::iter::repeat_n(b'x', 16_000));
        assert!(contains_in("latin1_long.txt", &no_nul, "caf\u{e9}"), "the same body without the NUL is Latin-1 text");
        assert!(!contains_in("utf8_lookalike.txt", "caf\u{c3}\u{a9}".as_bytes(), "caf\u{e9}"), "a UTF-8 file spelling caf\u{c3}\u{a9} does not contain caf\u{e9}");
    }

    #[test]
    fn contains_finds_substring_crossing_chunk_boundary() {
        // Force the substring to straddle a chunk boundary. The streaming
        // implementation uses an 8KiB chunk size internally; place the needle
        // so that part of it lies in the first chunk and the rest in the next.
        let chunk_size: usize = 8 * 1024;
        let needle = b"FOOBAR_BOUNDARY";
        let prefix = chunk_size - 5;
        let mut bytes = vec![b'a'; prefix];
        bytes.extend_from_slice(needle);
        bytes.extend(std::iter::repeat_n(b'b', 100));
        let path = write_temp_file("boundary.bin", &bytes);
        let entry = dir_entry_for(&path);

        let result = get_value(
            &Function::Contains,
            String::from("FOOBAR_BOUNDARY"),
            vec![],
            Some(&entry),
            &None,
        )
        .unwrap();
        assert!(result.to_bool(), "Contains must find substrings that span chunks");

        let _ = std::fs::remove_file(&path);
    }
}