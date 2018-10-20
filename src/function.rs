extern crate serde;

use std::fmt::Display;
use std::fmt::Formatter;
use std::fmt::Error;
use std::str::FromStr;

use serde::ser::{Serialize, Serializer};

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
}

impl FromStr for Function {
    type Err = String;

    fn from_str<'a>(s: &str) -> Result<Self, Self::Err> {
        let function = s.to_ascii_lowercase();

        match function.as_str() {
            "lower" => Ok(Function::Lower),
            "upper" => Ok(Function::Upper),
            "length" => Ok(Function::Length),

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