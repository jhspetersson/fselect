use std::ops::Index;

use regex::Captures;
use regex::Regex;

use crate::util::error_exit;

pub fn is_glob(s: &str) -> bool {
    s.contains("*") || s.contains('?')
}

pub fn convert_glob_to_pattern(s: &str) -> String {
    let string = s.to_string();
    let regex = Regex::new("(\\?|\\.|\\*|\\[|\\]|\\(|\\)|\\^|\\$)").unwrap();
    let string = regex.replace_all(&string, |c: &Captures| {
        match c.index(0) {
            "." => "\\.",
            "*" => ".*",
            "?" => ".",
            "[" => "\\[",
            "]" => "\\]",
            "(" => "\\(",
            ")" => "\\)",
            "^" => "\\^",
            "$" => "\\$",
            _ => error_exit("Error parsing glob expression", s),
        }
        .to_string()
    });

    format!("^(?i){}$", string)
}

pub fn convert_like_to_pattern(s: &str) -> String {
    let string = s.to_string();
    let regex = Regex::new("(%|_|\\?|\\.|\\*|\\[|\\]|\\(|\\)|\\^|\\$)").unwrap();
    let string = regex.replace_all(&string, |c: &Captures| {
        match c.index(0) {
            "%" => ".*",
            "_" => ".",
            "?" => ".?",
            "." => "\\.",
            "*" => "\\*",
            "[" => "\\[",
            "]" => "\\]",
            "(" => "\\(",
            ")" => "\\)",
            "^" => "\\^",
            "$" => "\\$",
            _ => error_exit("Error parsing LIKE expression", s),
        }
        .to_string()
    });

    format!("^(?i){}$", string)
}
