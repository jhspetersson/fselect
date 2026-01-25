use std::ops::Index;

use regex::Captures;
use regex::Regex;

pub fn is_glob(s: &str) -> bool {
    s.contains("*") || s.contains('?')
}

pub fn convert_glob_to_pattern(s: &str) -> Result<String, String> {
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
            _ => "",
        }
        .to_string()
    });

    if string.is_empty() {
        return Err("Error parsing glob expression: ".to_string() + s);
    }

    Ok(format!("^(?i){}$", string))
}

pub fn convert_like_to_pattern(s: &str) -> Result<String, String> {
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
            _ => "",
        }
        .to_string()
    });

    if string.is_empty() {
        return Err("Error parsing LIKE expression: ".to_string() + s);
    }

    Ok(format!("^(?i){}$", string))
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
        let pattern = convert_glob_to_pattern("*.txt").unwrap();
        assert_eq!(pattern, "^(?i).*\\.txt$");
    }

    #[test]
    fn test_convert_glob_to_pattern_question_mark() {
        let pattern = convert_glob_to_pattern("file?.txt").unwrap();
        assert_eq!(pattern, "^(?i)file.\\.txt$");
    }

    #[test]
    fn test_convert_glob_to_pattern_mixed() {
        let pattern = convert_glob_to_pattern("file-*.?xt").unwrap();
        assert_eq!(pattern, "^(?i)file-.*\\..xt$");
    }

    #[test]
    fn test_convert_glob_to_pattern_special_chars() {
        let pattern = convert_glob_to_pattern("file[1-3].txt").unwrap();
        assert_eq!(pattern, "^(?i)file\\[1-3\\]\\.txt$");
    }

    #[test]
    fn test_convert_like_to_pattern_percent() {
        let pattern = convert_like_to_pattern("%.txt").unwrap();
        assert_eq!(pattern, "^(?i).*\\.txt$");
    }

    #[test]
    fn test_convert_like_to_pattern_underscore() {
        let pattern = convert_like_to_pattern("file_.txt").unwrap();
        assert_eq!(pattern, "^(?i)file.\\.txt$");
    }

    #[test]
    fn test_convert_like_to_pattern_mixed() {
        let pattern = convert_like_to_pattern("file-%.txt").unwrap();
        assert_eq!(pattern, "^(?i)file-.*\\.txt$");
    }

    #[test]
    fn test_convert_like_to_pattern_question_mark() {
        let pattern = convert_like_to_pattern("file?.txt").unwrap();
        assert_eq!(pattern, "^(?i)file.?\\.txt$");
    }

    #[test]
    fn test_convert_like_to_pattern_special_chars() {
        let pattern = convert_like_to_pattern("file*.txt").unwrap();
        assert_eq!(pattern, "^(?i)file\\*\\.txt$");
    }
}
