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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_glob_with_asterisk() {
        assert!(is_glob("file*.txt"));
        assert!(is_glob("*file.txt"));
        assert!(is_glob("file.txt*"));
    }

    #[test]
    fn test_is_glob_with_question_mark() {
        assert!(is_glob("file?.txt"));
        assert!(is_glob("?file.txt"));
        assert!(is_glob("file.txt?"));
    }

    #[test]
    fn test_is_glob_with_no_glob_chars() {
        assert!(!is_glob("file.txt"));
        assert!(!is_glob("path/to/file.txt"));
        assert!(!is_glob(""));
    }

    #[test]
    fn test_convert_glob_to_pattern_asterisk() {
        let pattern = convert_glob_to_pattern("*.txt");
        assert_eq!(pattern, "^(?i).*\\.txt$");
    }

    #[test]
    fn test_convert_glob_to_pattern_question_mark() {
        let pattern = convert_glob_to_pattern("file?.txt");
        assert_eq!(pattern, "^(?i)file.\\.txt$");
    }

    #[test]
    fn test_convert_glob_to_pattern_mixed() {
        let pattern = convert_glob_to_pattern("file-*.?xt");
        assert_eq!(pattern, "^(?i)file-.*\\..xt$");
    }

    #[test]
    fn test_convert_glob_to_pattern_special_chars() {
        let pattern = convert_glob_to_pattern("file[1-3].txt");
        assert_eq!(pattern, "^(?i)file\\[1-3\\]\\.txt$");
    }

    #[test]
    fn test_convert_like_to_pattern_percent() {
        let pattern = convert_like_to_pattern("%.txt");
        assert_eq!(pattern, "^(?i).*\\.txt$");
    }

    #[test]
    fn test_convert_like_to_pattern_underscore() {
        let pattern = convert_like_to_pattern("file_.txt");
        assert_eq!(pattern, "^(?i)file.\\.txt$");
    }

    #[test]
    fn test_convert_like_to_pattern_mixed() {
        let pattern = convert_like_to_pattern("file-%.txt");
        assert_eq!(pattern, "^(?i)file-.*\\.txt$");
    }

    #[test]
    fn test_convert_like_to_pattern_question_mark() {
        let pattern = convert_like_to_pattern("file?.txt");
        assert_eq!(pattern, "^(?i)file.?\\.txt$");
    }

    #[test]
    fn test_convert_like_to_pattern_special_chars() {
        let pattern = convert_like_to_pattern("file*.txt");
        assert_eq!(pattern, "^(?i)file\\*\\.txt$");
    }
}
