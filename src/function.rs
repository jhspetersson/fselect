//extern crate serde;

use std::collections::HashMap;
use std::fmt::Display;
use std::fmt::Formatter;
use std::fmt::Error;
use std::str::FromStr;

use chrono::Datelike;
use serde::ser::{Serialize, Serializer};

use crate::util::parse_datetime;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Hash)]
pub enum Function {
    Lower,
    Upper,
    Length,

    Min,
    Max,
    Avg,
    Sum,
    Count,

    Day,
    Month,
    Year,
}

impl FromStr for Function {
    type Err = String;

    fn from_str<'a>(s: &str) -> Result<Self, Self::Err> {
        let function = s.to_ascii_lowercase();

        match function.as_str() {
            "lower" => Ok(Function::Lower),
            "upper" => Ok(Function::Upper),
            "length" => Ok(Function::Length),

            "day" => Ok(Function::Day),
            "month" => Ok(Function::Month),
            "year" => Ok(Function::Year),

            "min" => Ok(Function::Min),
            "max" => Ok(Function::Max),
            "avg" => Ok(Function::Avg),
            "sum" => Ok(Function::Sum),
            "count" => Ok(Function::Count),

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
}

pub fn get_value(function: &Option<Function>, function_arg: String) -> String {
    match function {
        Some(Function::Lower) => {
            return function_arg.to_lowercase();
        },
        Some(Function::Upper) => {
            return function_arg.to_uppercase();
        },
        Some(Function::Length) => {
            return format!("{}", function_arg.chars().count());
        },
        Some(Function::Year) => {
            match parse_datetime(&function_arg) {
                Ok(date) => {
                    return date.0.year().to_string();
                },
                _ => {
                    return String::new();
                }
            }
        },
        Some(Function::Month) => {
            match parse_datetime(&function_arg) {
                Ok(date) => {
                    return date.0.month().to_string();
                },
                _ => {
                    return String::new();
                }
            }
        },
        Some(Function::Day) => {
            match parse_datetime(&function_arg) {
                Ok(date) => {
                    return date.0.day().to_string();
                },
                _ => {
                    return String::new();
                }
            }
        },
        _ => {
            return String::new();
        }
    }
}

pub fn get_aggregate_value(function: &Option<Function>,
                           raw_output_buffer: &Vec<HashMap<String, String>>,
                           field_value: String,
                           default_value: &Option<String>) -> String {
    match function {
        Some(Function::Min) => {
            let mut min = -1;
            for value in raw_output_buffer {
                if let Some(value) = value.get(&field_value) {
                    if let Ok(value) = value.parse::<i64>() {
                        if value < min || min == -1 {
                            min = value;
                        }
                    }
                }
            }

            return min.to_string();
        },
        Some(Function::Max) => {
            let mut max = 0;
            for value in raw_output_buffer {
                if let Some(value) = value.get(&field_value) {
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
            let mut sum = 0;
            for value in raw_output_buffer {
                if let Some(value) = value.get(&field_value) {
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
                if let Some(value) = value.get(&field_value) {
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